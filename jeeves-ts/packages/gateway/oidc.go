package main

import (
	"context"
	"database/sql"
	"errors"
	"net/http"
	"net/url"
	"os"
	"time"

	"github.com/coreos/go-oidc/v3/oidc"
	"golang.org/x/oauth2"
	"jeeves/internal/cloudkit"
)

type signIn struct {
	oauth    oauth2.Config
	verifier *oidc.IDTokenVerifier
	apple    bool
}

func init() {
	schema = append(schema, `CREATE TABLE IF NOT EXISTS gw_oidc_states (
		id TEXT PRIMARY KEY, browser TEXT NOT NULL, verifier TEXT NOT NULL, nonce TEXT NOT NULL,
		owner TEXT NOT NULL, revision BIGINT NOT NULL, destination TEXT NOT NULL, expires BIGINT NOT NULL)`)
}
func configureSignIn(ctx context.Context, origin *url.URL, issuer string) (*signIn, error) {
	u, err := url.Parse(issuer)
	if err != nil || u.Scheme != "https" || u.Host == "" || u.User != nil {
		return nil, errors.New("OIDC issuer must use HTTPS")
	}
	id, secret := os.Getenv("GW_OIDC_CLIENT_ID"), os.Getenv("GW_OIDC_CLIENT_SECRET")
	if id == "" || secret == "" {
		return nil, errors.New("OIDC client ID and secret are required")
	}
	provider, err := oidc.NewProvider(ctx, issuer)
	if err != nil {
		return nil, err
	}
	apple := issuer == "https://appleid.apple.com"
	endpoint := provider.Endpoint()
	if apple {
		endpoint.AuthStyle = oauth2.AuthStyleInParams
	}
	return &signIn{
		oauth:    oauth2.Config{ClientID: id, ClientSecret: secret, Endpoint: endpoint, RedirectURL: origin.String() + "/api/oidc/callback", Scopes: []string{oidc.ScopeOpenID, "email"}},
		verifier: provider.Verifier(&oidc.Config{ClientID: id}), apple: apple,
	}, nil
}

const browserCookie = "__Host-jeeves_oauth"

func (g *gateway) oauth(w http.ResponseWriter, r *http.Request) {
	if g.oidc == nil {
		cloudkit.Error(w, 404, "single sign-on unavailable")
		return
	}
	if r.URL.Path == "/api/oidc/start" && r.Method == "GET" {
		g.beginOAuth(w, r)
		return
	}
	if r.URL.Path == "/api/oidc/callback" && (r.Method == "GET" || r.Method == "POST") {
		g.completeOAuth(w, r)
		return
	}
	cloudkit.Error(w, 405, "method not allowed")
}

func (g *gateway) beginOAuth(w http.ResponseWriter, r *http.Request) {
	ctx := r.Context()
	if !g.rate(w, r, "oauth:"+g.clientIP(r), 30) {
		return
	}
	state, browser, nonce := cloudkit.Token(24), cloudkit.Token(24), cloudkit.Token(24)
	verifier := oauth2.GenerateVerifier()
	var owner string
	var revision int64
	destination := "/app/"
	if r.URL.Query().Get("next") == "/device/" {
		destination = "/device/"
	}
	if r.URL.Query().Get("link") == "1" {
		ss, a, err := g.store.authenticate(ctx, g.sessionToken(r))
		if err != nil || time.Since(time.UnixMilli(ss.Issued)) > 10*time.Minute {
			cloudkit.Error(w, 403, "sign in again before linking")
			return
		}
		owner, revision = a.id, a.revision
		destination = "/account/"
	}
	_, err := g.store.db.ExecContext(ctx, `INSERT INTO gw_oidc_states(id,browser,verifier,nonce,owner,revision,destination,expires) VALUES($1,$2,$3,$4,$5,$6,$7,$8)`,
		g.store.digest(state), g.store.digest(browser), verifier, nonce, owner, revision, destination, time.Now().Add(10*time.Minute).UnixMilli())
	if err != nil {
		failure(w, err)
		return
	}
	http.SetCookie(w, &http.Cookie{Name: browserCookie, Value: browser, Path: "/", HttpOnly: true, Secure: true, SameSite: http.SameSiteNoneMode, MaxAge: 600})
	options := []oauth2.AuthCodeOption{oidc.Nonce(nonce)}
	if g.oidc.apple {
		options = append(options, oauth2.SetAuthURLParam("response_mode", "form_post"))
	} else {
		options = append(options, oauth2.S256ChallengeOption(verifier))
	}
	http.Redirect(w, r, g.oidc.oauth.AuthCodeURL(state, options...), 303)
}
func (g *gateway) completeOAuth(w http.ResponseWriter, r *http.Request) {
	ctx := r.Context()
	r.Body = http.MaxBytesReader(w, r.Body, 16<<10)
	if err := r.ParseForm(); err != nil {
		failure(w, rejected)
		return
	}
	browser, err := r.Cookie(browserCookie)
	if err != nil {
		failure(w, rejected)
		return
	}
	state := r.Form.Get("state")
	code := r.Form.Get("code")
	if len(state) != 48 || len(browser.Value) != 48 || code == "" || len(code) > 8192 {
		failure(w, rejected)
		return
	}
	var verifier, nonce, owner, destination string
	var revision int64
	err = g.store.db.QueryRowContext(ctx, `DELETE FROM gw_oidc_states WHERE id=$1 AND browser=$2 AND expires>$3
		RETURNING verifier,nonce,owner,revision,destination`, g.store.digest(state), g.store.digest(browser.Value), time.Now().UnixMilli()).Scan(&verifier, &nonce, &owner, &revision, &destination)
	if err != nil {
		failure(w, err)
		return
	}
	timed, end := context.WithTimeout(ctx, 12*time.Second)
	defer end()
	var options []oauth2.AuthCodeOption
	if !g.oidc.apple {
		options = append(options, oauth2.VerifierOption(verifier))
	}
	token, err := g.oidc.oauth.Exchange(timed, code, options...)
	if err != nil {
		failure(w, rejected)
		return
	}
	raw, _ := token.Extra("id_token").(string)
	id, err := g.oidc.verifier.Verify(timed, raw)
	if err != nil || id.Nonce != nonce || id.Subject == "" {
		failure(w, rejected)
		return
	}
	var claims struct {
		Email    string `json:"email"`
		Verified any    `json:"email_verified"`
	}
	if err := id.Claims(&claims); err != nil {
		failure(w, rejected)
		return
	}
	a, err := g.oauthUser(ctx, id.Issuer, id.Subject, claims.Email, claims.Verified == true || g.oidc.apple && claims.Verified == "true", owner, revision)
	if err != nil {
		failure(w, err)
		return
	}
	if a.factor != "" && owner == "" {
		http.Redirect(w, r, "/account/?next="+url.QueryEscape(destination)+"&factor=required", 303)
		return
	}
	if owner == "" {
		session, err := g.store.session(ctx, a, clientName(r))
		if err != nil {
			failure(w, err)
			return
		}
		g.cookie(w, session, 14*86400)
	}
	http.Redirect(w, r, destination, 303)
}
func (g *gateway) oauthUser(ctx context.Context, issuer, subject, email string, verified bool, owner string, revision int64) (user, error) {
	var a user
	err := g.transaction(ctx, func(tx *sql.Tx) error {
		if owner != "" {
			var err error
			a, err = scanUser(tx.QueryRowContext(ctx, "SELECT "+userColumns+" FROM gw_users WHERE id=$1 AND revision=$2 AND verified", owner, revision))
			if err != nil {
				return err
			}
			_, err = tx.ExecContext(ctx, "INSERT INTO gw_oidc(issuer,subject,owner) VALUES($1,$2,$3)", issuer, subject, owner)
			return err
		}
		u, err := scanUser(tx.QueryRowContext(ctx, "SELECT "+userColumns+" FROM gw_users WHERE id=(SELECT owner FROM gw_oidc WHERE issuer=$1 AND subject=$2)", issuer, subject))
		if err == nil {
			a = u
			return nil
		}
		if !errors.Is(err, sql.ErrNoRows) {
			return err
		}
		email = normalizeEmail(email)
		if !verified || len(email) > 254 || !emailPattern.MatchString(email) {
			return rejected
		}
		a, err = g.store.create(ctx, tx, email, "", "", true)
		if err != nil {
			return err
		}
		_, err = tx.ExecContext(ctx, "INSERT INTO gw_oidc(issuer,subject,owner) VALUES($1,$2,$3)", issuer, subject, a.id)
		return err
	})
	return a, err
}
