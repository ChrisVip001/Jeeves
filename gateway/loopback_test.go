package main

import (
	"net/http"
	"net/http/httptest"
	"net/url"
	"testing"
)

func TestLoopbackAliasesOfThePersonalGatewayRedirect(t *testing.T) {
	g := testGateway(t)
	g.origin = &url.URL{Scheme: "http", Host: "127.0.0.1:8791"}
	g.local = true
	visit := func(method, host string) *httptest.ResponseRecorder {
		r := httptest.NewRequest(method, "http://"+host+"/app/u-1/?list=1", nil)
		r.Host = host
		w := httptest.NewRecorder()
		g.serve(w, r)
		return w
	}

	for _, host := range []string{"localhost:8791", "LOCALHOST:8791", "[::1]:8791"} {
		w := visit("GET", host)
		if w.Code != http.StatusPermanentRedirect || w.Header().Get("Location") != "http://127.0.0.1:8791/app/u-1/?list=1" {
			t.Errorf("%s: got %d to %q", host, w.Code, w.Header().Get("Location"))
		}
	}
	for name, w := range map[string]*httptest.ResponseRecorder{
		"a write":        visit("POST", "localhost:8791"),
		"another port":   visit("GET", "localhost:9999"),
		"a foreign host": visit("GET", "evil.test:8791"),
		"no port":        visit("GET", "localhost"),
	} {
		if w.Code != http.StatusForbidden {
			t.Errorf("%s: got %d, want 403", name, w.Code)
		}
	}

	g.local = false
	if w := visit("GET", "localhost:8791"); w.Code != http.StatusForbidden {
		t.Errorf("a hosted gateway must not redirect, got %d", w.Code)
	}
}
