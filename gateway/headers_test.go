package main

import (
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
)

func TestEveryResponseDeniesUnusedBrowserFeatures(t *testing.T) {
	g := testGateway(t)
	g.origin = &url.URL{Scheme: "https", Host: "gateway.test"}
	for _, path := range []string{"/healthz", "/app/u-1/", "/nowhere"} {
		w := httptest.NewRecorder()
		g.serve(w, httptest.NewRequest("GET", "http://gateway.test"+path, nil))
		policy := w.Header().Get("Permissions-Policy")
		for _, feature := range []string{"camera=()", "microphone=()", "geolocation=()"} {
			if !strings.Contains(policy, feature) {
				t.Errorf("%s: Permissions-Policy %q lacks %s", path, policy, feature)
			}
		}
	}
}
