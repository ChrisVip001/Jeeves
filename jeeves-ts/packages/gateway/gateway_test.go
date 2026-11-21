package main

import (
	"bytes"
	"context"
	"database/sql"
	"encoding/json"
	"io"
	"net/http"
	"net/http/cookiejar"
	"net/http/httptest"
	"net/url"
	"path/filepath"
	"testing"
	"time"

	"jeeves/internal/cloudkit"
)

func testGateway(t *testing.T) *gateway {
	t.Helper()
	ctx := context.Background()
	db, err := cloudkit.Open(ctx, "sqlite:"+filepath.Join(t.TempDir(), "gateway.sqlite"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { db.Close() })
	s, err := newStore(ctx, db, bytes.Repeat([]byte("s"), 32), bytes.Repeat([]byte("k"), 32))
	if err != nil {
		t.Fatal(err)
	}
	return &gateway{store: s, http: &http.Client{Timeout: time.Second}}
}
func TestAuthenticatedTenantRouting(t *testing.T) {
	g := testGateway(t)
	ctx := context.Background()
	var owner user
	err := g.transaction(ctx, func(tx *sql.Tx) error {
		var err error
		owner, err = g.store.create(ctx, tx, "owner@example.test", "", "", true)
		return err
	})
	if err != nil {
		t.Fatal(err)
	}
	rt := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/api/data" || cloudkit.Bearer(r) != "runtime-machine-token" || r.Header.Get("Cookie") != "" {
			t.Error("upstream got wrong identity or path")
		}
		w.Header().Set("Set-Cookie", "upstream=unexpected")
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, "data: progress\n\n")
	}))
	defer rt.Close()
	cp := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !cloudkit.Authorized(r, "gateway-service-token") {
			t.Error("control plane got wrong credential")
		}
		var route cloudkit.RuntimeRequest
		if json.NewDecoder(r.Body).Decode(&route) != nil || route.Tenant != owner.id {
			t.Error("incorrect tenant routed")
		}
		cloudkit.JSON(w, 200, cloudkit.RuntimeEndpoint{URL: rt.URL, Token: "runtime-machine-token", Ready: true})
	}))
	defer cp.Close()
	g.controlURL, g.controlToken = cp.URL, "gateway-service-token"
	server := httptest.NewTLSServer(g.handler())
	defer server.Close()
	g.origin, _ = url.Parse(server.URL)
	client := server.Client()
	client.Jar, _ = cookiejar.New(nil)
	token, err := g.store.session(ctx, owner, "Test")
	if err != nil {
		t.Fatal(err)
	}
	client.Jar.SetCookies(g.origin, []*http.Cookie{{Name: g.cookieName(), Value: token, Path: "/", Secure: true}})
	for _, item := range []struct {
		path   string
		status int
	}{
		{"/app/" + owner.id + "/api/data", 200},
		{"/app/u-000000000000/api/data", 403},
		{"/app/" + owner.id + "/api/gateway/rpc", 404},
	} {
		reply, err := client.Get(server.URL + item.path)
		if err != nil {
			t.Fatal(err)
		}
		b, err := io.ReadAll(reply.Body)
		reply.Body.Close()
		if err != nil || reply.StatusCode != item.status {
			t.Fatalf("%s: %d %s", item.path, reply.StatusCode, b)
		}
		if item.status == 200 && (reply.Header.Get("X-Accel-Buffering") != "no" || reply.Header.Get("Set-Cookie") != "" || string(b) != "data: progress\n\n") {
			t.Fatal("stream was changed by the gateway")
		}
	}
}
