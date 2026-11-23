package main

import (
	"context"
	"errors"
	"net"
	"net/http"
	"net/http/httptest"
	"testing"
)

func refusedDial(t *testing.T) error {
	t.Helper()
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	addr := listener.Addr().String()
	listener.Close()
	_, err = http.Get("http://" + addr + "/")
	if err == nil {
		t.Fatal("dialing a closed port should fail")
	}
	return err
}

func TestARestartingRuntimeReadsAsStarting(t *testing.T) {
	w := httptest.NewRecorder()
	proxyFailure(w, refusedDial(t))
	if w.Code != 503 || w.Header().Get("Retry-After") != "2" {
		t.Fatalf("refused connection: got %d Retry-After %q", w.Code, w.Header().Get("Retry-After"))
	}
	if body := w.Body.String(); body != "{\"error\":\"workspace is starting\"}\n" {
		t.Fatalf("body %q", body)
	}
}

func TestOtherProxyFailuresStayInterrupted(t *testing.T) {
	w := httptest.NewRecorder()
	proxyFailure(w, errors.New("upstream reset"))
	if w.Code != 502 || w.Header().Get("Retry-After") != "" {
		t.Fatalf("got %d Retry-After %q", w.Code, w.Header().Get("Retry-After"))
	}
	gone := httptest.NewRecorder()
	proxyFailure(gone, context.Canceled)
	if gone.Body.Len() != 0 {
		t.Fatal("a client that left gets nothing written")
	}
}
