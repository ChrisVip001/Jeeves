package cloudkit

import (
	"context"
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"net/http"
	"net/url"
	"os"
	"os/signal"
	"regexp"
	"strings"
	"syscall"
	"time"
)

var TenantPattern = regexp.MustCompile(`^u-[a-f0-9]{12}$`)

func Token(bytes int) string {
	return hex.EncodeToString(randBytes(bytes))
}

func randBytes(n int) []byte {
	bytes := make([]byte, n)
	rand.Read(bytes)
	return bytes
}

func Digest(value string) string {
	hash := sha256.Sum256([]byte(value))
	return hex.EncodeToString(hash[:])
}

func MAC(key []byte, value string) []byte {
	h := hmac.New(sha256.New, key)
	h.Write([]byte(value))
	return h.Sum(nil)
}

func Bearer(r *http.Request) string {
	v, ok := strings.CutPrefix(r.Header.Get("Authorization"), "Bearer ")
	if !ok || len(v) > 256 {
		return ""
	}
	return v
}

func Authorized(r *http.Request, key string) bool {
	token := Bearer(r)
	return key != "" && token != "" && hmac.Equal([]byte(Digest(token)), []byte(Digest(key)))
}

func JSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	if err := json.NewEncoder(w).Encode(v); err != nil {
		log.Printf("write response: %v", err)
	}
}

func Error(w http.ResponseWriter, status int, message string) {
	JSON(w, status, map[string]string{"error": message})
}

func Decode(w http.ResponseWriter, r *http.Request, max int64, value any) error {
	if strings.Split(r.Header.Get("Content-Type"), ";")[0] != "application/json" {
		return errors.New("expected a JSON request")
	}
	reader := json.NewDecoder(http.MaxBytesReader(w, r.Body, max))
	if err := reader.Decode(value); err != nil {
		return err
	}
	if err := reader.Decode(new(any)); err != io.EOF {
		return errors.New("unexpected data after the request")
	}
	return nil
}

func Origin(raw string) (*url.URL, error) {
	value, err := url.Parse(raw)
	if err != nil || value.Host == "" || value.User != nil || value.Opaque != "" ||
		(value.Scheme != "http" && value.Scheme != "https") ||
		(value.Path != "/" && value.Path != "") || value.RawQuery != "" || value.Fragment != "" {
		return nil, fmt.Errorf("invalid service origin")
	}
	value.Path = ""
	return value, nil
}

func Serve(addr, cert, key string, makeHandler func(context.Context) http.Handler) error {
	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer stop()
	run, cancel := context.WithCancel(context.Background())
	defer cancel()
	server := &http.Server{
		Addr: addr, Handler: makeHandler(run), ReadHeaderTimeout: 10 * time.Second,
		IdleTimeout: 90 * time.Second, MaxHeaderBytes: 64 << 10,
	}
	finished := make(chan error, 1)
	go func() {
		if cert != "" || key != "" {
			finished <- server.ListenAndServeTLS(cert, key)
		} else {
			finished <- server.ListenAndServe()
		}
	}()
	select {
	case err := <-finished:
		return err
	case <-ctx.Done():
		cancel()
		drain, stop := context.WithTimeout(context.Background(), 10*time.Second)
		defer stop()
		return server.Shutdown(drain)
	}
}

func Env(name, fallback string) string {
	if value := os.Getenv(name); value != "" {
		return value
	}
	return fallback
}
