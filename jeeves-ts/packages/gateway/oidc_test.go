package main

import (
	"context"
	"crypto/rand"
	"crypto/rsa"
	"crypto/sha256"
	"database/sql"
	"encoding/base64"
	"net/http"
	"net/http/cookiejar"
	"net/http/httptest"
	"net/url"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/coreos/go-oidc/v3/oidc"
	"github.com/go-jose/go-jose/v4"
	"github.com/go-jose/go-jose/v4/jwt"
	"jeeves/internal/cloudkit"
)

func TestOIDCCallbackAndAccountLinking(t *testing.T) {
	ctx := context.Background()
	g := testGateway(t)
	var existing user
	err := g.transaction(ctx, func(tx *sql.Tx) error {
		var err error
		existing, err = g.store.create(ctx, tx, "existing@example.test", "", "", true)
		return err
	})
	if err != nil {
		t.Fatal(err)
	}
	key, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		t.Fatal(err)
	}
	public := jose.JSONWebKey{Key: &key.PublicKey, KeyID: "test-key", Algorithm: string(jose.RS256), Use: "sig"}
	signer, err := jose.NewSigner(jose.SigningKey{Algorithm: jose.RS256, Key: jose.JSONWebKey{Key: key, KeyID: public.KeyID}}, nil)
	if err != nil {
		t.Fatal(err)
	}
	var expectedChallenge atomic.Value
	var providerURL string
	provider := httptest.NewUnstartedServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/.well-known/openid-configuration":
			cloudkit.JSON(w, 200, map[string]any{
				"issuer": providerURL, "authorization_endpoint": providerURL + "/authorize",
				"token_endpoint": providerURL + "/token", "jwks_uri": providerURL + "/jwks",
				"id_token_signing_alg_values_supported": []string{"RS256"},
			})
		case "/jwks":
			cloudkit.JSON(w, 200, jose.JSONWebKeySet{Keys: []jose.JSONWebKey{public}})
		case "/token":
			checksum := sha256.Sum256([]byte(r.FormValue("code_verifier")))
			if base64.RawURLEncoding.EncodeToString(checksum[:]) != expectedChallenge.Load() {
				t.Error("token exchange lost its PKCE verifier")
			}
			subject, nonce, _ := strings.Cut(r.FormValue("code"), ".")
			email := subject + "@example.test"
			token, err := jwt.Signed(signer).
				Claims(jwt.Claims{Issuer: providerURL, Subject: subject, Audience: jwt.Audience{"jeeves-client"},
					IssuedAt: jwt.NewNumericDate(time.Now()), Expiry: jwt.NewNumericDate(time.Now().Add(time.Minute))}).
				Claims(map[string]any{"email": email, "email_verified": true, "nonce": nonce}).Serialize()
			if err != nil {
				t.Error(err)
				w.WriteHeader(500)
				return
			}
			cloudkit.JSON(w, 200, map[string]string{"access_token": "fixture", "token_type": "Bearer", "id_token": token})
		default:
			w.WriteHeader(404)
		}
	}))
	providerURL = "https://" + provider.Listener.Addr().String()
	provider.StartTLS()
	defer provider.Close()
	server := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		g.serve(w, r.WithContext(oidc.ClientContext(r.Context(), provider.Client())))
	}))
	defer server.Close()
	g.origin, err = url.Parse(server.URL)
	if err != nil {
		t.Fatal(err)
	}
	t.Setenv("GW_OIDC_CLIENT_ID", "jeeves-client")
	t.Setenv("GW_OIDC_CLIENT_SECRET", "jeeves-secret")
	ctx = oidc.ClientContext(ctx, provider.Client())
	g.oidc, err = configureSignIn(ctx, g.origin, provider.URL)
	if err != nil {
		t.Fatal(err)
	}
	client := server.Client()
	client.Jar, _ = cookiejar.New(nil)
	client.CheckRedirect = func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }
	request := func(method, path, origin string) *http.Response {
		t.Helper()
		r, err := http.NewRequestWithContext(ctx, method, server.URL+path, nil)
		if err != nil {
			t.Fatal(err)
		}
		if origin != "" {
			r.Header.Set("Origin", origin)
		}
		res, err := client.Do(r)
		if err != nil {
			t.Fatal(err)
		}
		res.Body.Close()
		return res
	}
	callback := func(method, subject, nonceOverride string) string {
		t.Helper()
		start := request(method, "/api/oidc/start?next=%2Fdevice%2F", server.URL)
		redirect, err := start.Location()
		if err != nil || start.StatusCode != 303 || redirect.Host != provider.Listener.Addr().String() {
			t.Fatalf("invalid provider redirect %v (%d): %v", redirect, start.StatusCode, err)
		}
		claims := redirect.Query()
		expectedChallenge.Store(claims.Get("code_challenge"))
		nonce := claims.Get("nonce")
		if nonceOverride != "" {
			nonce = nonceOverride
		}
		return "/api/oidc/callback?" + url.Values{"state": {claims.Get("state")}, "code": {subject + "." + nonce}}.Encode()
	}
	if wrong := request("GET", callback("GET", "unexpected", "wrong-nonce"), ""); wrong.StatusCode != 403 {
		t.Fatalf("wrong nonce returned %d", wrong.StatusCode)
	}
	if joined := request("GET", callback("GET", "existing", ""), ""); joined.StatusCode != 403 {
		t.Fatalf("matching email automatically linked with %d", joined.StatusCode)
	}
	token, err := g.store.session(ctx, existing, "test")
	if err != nil {
		t.Fatal(err)
	}
	client.Jar.SetCookies(g.origin, []*http.Cookie{{Name: g.cookieName(), Value: token, Secure: true, Path: "/"}})
	if forged := request("POST", "/api/oidc/start", "https://foreign.example.test"); forged.StatusCode != 403 {
		t.Fatalf("cross-origin account link returned %d", forged.StatusCode)
	}
	if linked := request("GET", callback("POST", "existing", ""), ""); linked.StatusCode != 303 || linked.Header.Get("Location") != "/account/" {
		t.Fatalf("explicit account link returned %d", linked.StatusCode)
	}
	replay := callback("GET", "existing", "")
	if ok := request("GET", replay, ""); ok.StatusCode != 303 || ok.Header.Get("Location") != "/device/" {
		t.Fatalf("linked sign-in returned %d or lost its destination", ok.StatusCode)
	}
	if duplicate := request("GET", replay, ""); duplicate.StatusCode != 403 {
		t.Fatalf("replayed OAuth state returned %d", duplicate.StatusCode)
	}
}
