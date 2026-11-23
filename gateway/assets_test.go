package main

import (
	"compress/gzip"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestBundleAssetsTravelGzippedAndRevalidate(t *testing.T) {
	dir := t.TempDir()
	body := strings.Repeat("console.log('jeeves');\n", 400)
	if err := os.WriteFile(filepath.Join(dir, "main.js"), []byte(body), 0o600); err != nil {
		t.Fatal(err)
	}
	g := &gateway{bundle: dir}
	get := func(header map[string]string) *httptest.ResponseRecorder {
		r := httptest.NewRequest("GET", "/app/main.js", nil)
		for k, v := range header {
			r.Header.Set(k, v)
		}
		w := httptest.NewRecorder()
		g.asset(w, r, "main.js")
		return w
	}

	zipped := get(map[string]string{"Accept-Encoding": "gzip, br"})
	if zipped.Code != 200 || zipped.Header().Get("Content-Encoding") != "gzip" {
		t.Fatalf("want gzip 200, got %d %q", zipped.Code, zipped.Header().Get("Content-Encoding"))
	}
	if zipped.Body.Len() >= len(body)/4 {
		t.Fatalf("gzip body is %d bytes for %d plain", zipped.Body.Len(), len(body))
	}
	zr, err := gzip.NewReader(zipped.Body)
	if err != nil {
		t.Fatal(err)
	}
	plain, err := io.ReadAll(zr)
	if err != nil || string(plain) != body {
		t.Fatal("gzip body does not round-trip to the file")
	}
	if got := zipped.Header().Get("Cache-Control"); got != "private, no-cache" {
		t.Fatalf("cache-control %q", got)
	}
	if !strings.Contains(zipped.Header().Get("Vary"), "Accept-Encoding") {
		t.Fatal("compressed asset must vary on Accept-Encoding")
	}

	again := get(map[string]string{"Accept-Encoding": "gzip", "If-None-Match": zipped.Header().Get("ETag")})
	if again.Code != http.StatusNotModified || again.Body.Len() != 0 {
		t.Fatalf("revalidation should be a bodiless 304, got %d with %d bytes", again.Code, again.Body.Len())
	}

	raw := get(map[string]string{"Accept-Encoding": "gzip;q=0"})
	if raw.Header().Get("Content-Encoding") != "" || raw.Body.String() != body {
		t.Fatal("q=0 must get the plain file")
	}
	if raw.Header().Get("ETag") == zipped.Header().Get("ETag") {
		t.Fatal("plain and gzip copies need different validators")
	}
}

func TestAcceptEncodingParsing(t *testing.T) {
	for header, want := range map[string]bool{
		"":                  false,
		"gzip":              true,
		"br, GZIP":          true,
		"gzip;q=0":          false,
		"gzip; q=0.000":     false,
		"gzip;q=0.5":        true,
		"deflate, identity": false,
		"gzip;q=nonsense":   false,
		"x-gzip, deflate":   false,
	} {
		r := httptest.NewRequest("GET", "/", nil)
		r.Header.Set("Accept-Encoding", header)
		if got := accepts(r, "gzip"); got != want {
			t.Errorf("accepts(%q) = %v, want %v", header, got, want)
		}
	}
}
