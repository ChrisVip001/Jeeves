package main

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"log"
	"net"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"time"

	"jeeves/internal/cloudkit"
)

type gateway struct {
	store                     *store
	origin                    *url.URL
	bundle                    string
	local, trustProxy         bool
	loopbackOnly              bool
	setup                     string
	fixed                     cloudkit.RuntimeEndpoint
	runtimeHome, runtimeToken string
	fixedOwner                string
	controlURL, controlToken  string
	http                      *http.Client
	mail                      func(context.Context, mailJob) error
	mailJobs                  chan mailJob
	oidc                      *signIn
	shutdown                  context.Context
}

func configuration(ctx context.Context) (*gateway, error) {
	origin, err := cloudkit.Origin(os.Getenv("GW_PUBLIC_ORIGIN"))
	if err != nil {
		return nil, fmt.Errorf("configure GW_PUBLIC_ORIGIN: %w", err)
	}
	local := os.Getenv("GW_LOCAL") == "1"
	if (!local && origin.Scheme != "https") || (local && !loopback(origin.Hostname())) {
		return nil, errors.New("use an HTTPS public origin or a loopback origin for personal mode")
	}
	fixed := os.Getenv("GW_RUNTIME_URL")
	runtimeHome := os.Getenv("GW_RUNTIME_HOME")
	privateRuntime := fixed != "" || runtimeHome != ""
	if runtimeHome != "" && (!filepath.IsAbs(runtimeHome) || !loopback(origin.Hostname()) || fixed != "" || os.Getenv("GW_CONTROL_URL") != "") {
		return nil, errors.New("GW_RUNTIME_HOME requires an absolute path and a loopback gateway without other Runtime routing")
	}
	owner := normalizeEmail(os.Getenv("GW_FIXED_OWNER_EMAIL"))
	secureFixed := !local && loopback(origin.Hostname()) && privateRuntime && emailPattern.MatchString(owner)
	dsn := os.Getenv("GW_DATABASE_URL")
	if dsn == "" || (!local && !secureFixed && strings.HasPrefix(dsn, "sqlite:") && os.Getenv("GW_DEVELOPMENT") != "1") {
		return nil, errors.New("set GW_DATABASE_URL to PostgreSQL, or use SQLite in personal mode")
	}
	db, err := cloudkit.Open(ctx, dsn)
	if err != nil {
		return nil, err
	}
	accounts, err := newStore(ctx, db, []byte(os.Getenv("GW_SESSION_KEY")), []byte(os.Getenv("GW_STATE_KEY")))
	if err != nil {
		db.Close()
		return nil, err
	}
	g := &gateway{store: accounts, origin: origin, local: local, loopbackOnly: local || secureFixed, trustProxy: os.Getenv("GW_TRUST_PROXY") == "1", setup: os.Getenv("GW_SETUP_TOKEN"),
		bundle:   cloudkit.Env("GW_DASHBOARD_DIR", "web/dist"),
		http:     &http.Client{Timeout: 5 * time.Second, CheckRedirect: cloudkit.RejectRedirect},
		mailJobs: make(chan mailJob, 32),
	}
	for _, entry := range []string{"/index.html", "/account/account.html"} {
		if _, err := os.Stat(g.bundle + entry); err != nil {
			return nil, fmt.Errorf("dashboard is missing; run just dashboard-build: %w", err)
		}
	}
	if privateRuntime {
		token := os.Getenv("GW_RUNTIME_TOKEN")
		if len(token) < 16 {
			return nil, errors.New("the private Runtime requires GW_RUNTIME_TOKEN")
		}
		g.fixedOwner = owner
		if !local && !emailPattern.MatchString(g.fixedOwner) {
			return nil, errors.New("a fixed cloud Runtime needs GW_FIXED_OWNER_EMAIL")
		}
		if local && !strings.HasPrefix(dsn, "sqlite:") {
			return nil, errors.New("personal mode uses a local SQLite account store")
		}
		if runtimeHome != "" {
			g.runtimeHome, g.runtimeToken = runtimeHome, token
		} else {
			u, err := cloudkit.Origin(fixed)
			if err != nil || u.Scheme != "https" && !loopback(u.Hostname()) {
				return nil, errors.New("a fixed Runtime requires a server-set HTTPS or loopback origin")
			}
			g.fixed = cloudkit.RuntimeEndpoint{URL: u.String(), Token: token, Ready: true}
		}
	} else {
		u, err := cloudkit.Origin(os.Getenv("GW_CONTROL_URL"))
		if local || err != nil || len(os.Getenv("GW_CONTROL_TOKEN")) < 32 {
			return nil, errors.New("configure private control-plane origin and service token")
		}
		g.controlURL, g.controlToken = u.String(), os.Getenv("GW_CONTROL_TOKEN")
	}
	if os.Getenv("GW_SMTP_URL") != "" || os.Getenv("GW_RESEND_API_KEY") != "" {
		g.mail, err = configureMail(origin)
		if err != nil {
			return nil, err
		}
	}
	if issuer := os.Getenv("GW_OIDC_ISSUER"); issuer != "" {
		if local || privateRuntime {
			return nil, errors.New("OIDC requires the managed cloud workspace mode")
		}
		g.oidc, err = configureSignIn(ctx, origin, issuer)
		if err != nil {
			return nil, fmt.Errorf("OIDC: %w", err)
		}
	}
	return g, nil
}
func loopback(host string) bool {
	if host == "localhost" {
		return true
	}
	ip := net.ParseIP(host)
	return ip != nil && ip.IsLoopback()
}

func main() {
	if executorCLI() {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	g, err := configuration(ctx)
	cancel()
	if err != nil {
		log.Fatal(err)
	}
	defer g.store.db.Close()
	addr := cloudkit.Env("GW_ADDR", ":8080")
	if g.loopbackOnly {
		host, _, e := net.SplitHostPort(addr)
		if e != nil || !loopback(host) {
			log.Fatal("personal gateway must listen on loopback")
		}
	}
	cert, key := os.Getenv("GW_TLS_CERT"), os.Getenv("GW_TLS_KEY")
	if !g.local && cert == "" && !g.trustProxy {
		log.Fatal("configure HTTPS or trust the private cloud ingress")
	}
	if err := cloudkit.Serve(addr, cert, key, func(ctx context.Context) http.Handler {
		g.shutdown = ctx
		go g.mailWorker(ctx)
		go g.cleanup(ctx)
		log.Printf("Jeeves gateway listening on %s", addr)
		return g.handler()
	}); err != nil {
		log.Fatal(err)
	}
}
func (g *gateway) cleanup(ctx context.Context) {
	ticker := time.NewTicker(time.Minute)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			now := time.Now()
			for _, table := range []string{"gw_sessions", "gw_challenges", "gw_pending_factors", "gw_oidc_states", "gw_mac_links", "gw_mac_operations"} {
				if _, err := g.store.db.ExecContext(ctx, "DELETE FROM "+table+" WHERE expires<=$1", now.UnixMilli()); err != nil {
					log.Printf("expiry cleanup failed for %s (%T)", table, err)
				}
			}
			if _, err := g.store.db.ExecContext(ctx, "DELETE FROM gw_macs WHERE owner IS NULL AND expires<=$1", now.UnixMilli()); err != nil {
				log.Printf("device claim cleanup failed (%T)", err)
			}
			if _, err := g.store.db.ExecContext(ctx, `DELETE FROM gw_rates WHERE "window"<$1`, now.Unix()/60-2); err != nil {
				log.Printf("rate cleanup failed (%T)", err)
			}
		}
	}
}

func (g *gateway) handler() http.Handler { return http.HandlerFunc(g.serve) }
func (g *gateway) serve(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("Referrer-Policy", "no-referrer")
	w.Header().Set("X-Content-Type-Options", "nosniff")
	w.Header().Set("X-Frame-Options", "DENY")
	// The cockpit uses none of these; denying them means injected script cannot prompt for them.
	w.Header().Set("Permissions-Policy", "camera=(), microphone=(), geolocation=(), payment=(), usb=()")
	if r.URL.Path == "/healthz" && r.Method == "GET" {
		ctx, cancel := context.WithTimeout(r.Context(), 2*time.Second)
		defer cancel()
		if err := g.store.db.PingContext(ctx); err != nil {
			failure(w, err)
			return
		}
		status := map[string]bool{"ok": true}
		if g.runtimeHome != "" {
			target, err := g.discover(ctx)
			status["runtimeReady"] = err == nil && target.Ready
		}
		cloudkit.JSON(w, 200, status)
		return
	}
	if r.URL.Path == "/internal/desktop" && r.Method == "POST" && !g.local {
		g.relay(w, r)
		return
	}

	if !strings.EqualFold(r.Host, g.origin.Host) || (!g.local && r.TLS == nil && !(g.trustProxy && r.Header.Get("X-Forwarded-Proto") == "https")) {
		if g.local && (r.Method == "GET" || r.Method == "HEAD") && loopbackAlias(r.Host, g.origin) {
			target := *g.origin
			target.Path, target.RawQuery = r.URL.Path, r.URL.RawQuery
			http.Redirect(w, r, target.String(), http.StatusPermanentRedirect)
			return
		}
		cloudkit.Error(w, 403, "use the configured gateway")
		return
	}
	if !g.local {
		w.Header().Set("Strict-Transport-Security", "max-age=31536000")
	}
	method, path := r.Method, r.URL.Path
	read := method == "GET" || method == "HEAD"
	isCallback := path == "/api/oidc/callback"
	if !read && !isCallback && (r.Header.Get("Sec-Fetch-Site") == "cross-site" ||
		(r.Header.Get("Origin") != "" && r.Header.Get("Origin") != g.origin.String()) ||
		(r.Header.Get("Origin") == "" && !strings.HasPrefix(r.Header.Get("Content-Type"), "application/json") && r.Header.Get("X-Jeeves-Client") != "1")) {
		cloudkit.Error(w, 403, "request origin rejected")
		return
	}
	if read && path == "/account" {
		http.Redirect(w, r, "/account/?next=%2Fapp%2F", http.StatusSeeOther)
		return
	}
	if read && strings.HasPrefix(path, "/account/") {
		name := strings.TrimPrefix(path, "/account/")
		if name == "" || name == "index.html" {
			name = "account.html"
		}
		g.asset(w, r, "account/"+name)
		return
	}
	if g.deviceRoute(w, r) {
		return
	}

	if path == "/api/oidc/start" || isCallback {
		g.oauth(w, r)
		return
	}
	if method == "POST" {
		switch path {
		case "/api/register", "/api/login", "/api/send-verification", "/api/forgot", "/api/reset", "/api/verify", "/api/setup":
			g.identity(w, r)
			return
		}
	}
	token := g.sessionToken(r)
	ss, a, err := g.store.authenticate(r.Context(), token)
	if err == nil && g.fixedOwner != "" && a.email != g.fixedOwner {
		err = rejected
	}
	if err != nil {
		if !errors.Is(err, sql.ErrNoRows) && !errors.Is(err, rejected) {
			failure(w, err)
			return
		}
		if read && path == "/" {
			http.Redirect(w, r, "/account/?next=%2Fapp%2F", http.StatusSeeOther)
			return
		}
		if read && (path == "/app" || strings.HasPrefix(path, "/app/")) && strings.Contains(r.Header.Get("Accept"), "text/html") {
			http.Redirect(w, r, "/account/?next="+url.QueryEscape(r.URL.RequestURI()), 303)
			return
		}
		w.Header().Set("X-Jeeves-Session", "required")
		cloudkit.JSON(w, 401, map[string]any{"error": "sign in first", "signup": !g.local, "oidc": g.oidc != nil})
		return
	}
	if !g.local && ((method == "GET" && path == "/api/mac/devices") || (method == "DELETE" && strings.HasPrefix(path, "/api/mac/devices/"))) {
		g.devices(w, r, a)
		return
	}
	if method == "GET" && path == "/api/account/usage" && g.controlURL != "" {
		var usage cloudkit.ModelUsage
		if err := g.controlTenant(r.Context(), "/internal/usage", a.id, &usage); err != nil {
			failure(w, err)
			return
		}
		cloudkit.JSON(w, 200, usage)
		return
	}

	if path == "/api/account" && method == "GET" {
		sessions, err := g.store.list(r.Context(), a.id)
		if err != nil {
			failure(w, err)
			return
		}
		cloudkit.JSON(w, 200, map[string]any{"email": a.email, "factorEnabled": a.factor != "", "createdAt": a.created, "app": "/app/" + a.id + "/",
			"current": ss.ID, "sessions": sessions, "devices": !g.local && g.controlURL != "", "managed": g.controlURL != "", "oidc": g.oidc != nil, "passwordSet": a.hash != ""})
		return
	}
	if (method == "POST" && (path == "/api/logout" || path == "/api/account/password" || strings.HasPrefix(path, "/api/account/factor/"))) ||
		(method == "DELETE" && strings.HasPrefix(path, "/api/account/sessions/")) {
		g.accountMutation(w, r, a, ss)
		return
	}
	if read && (path == "/" || path == "/app" || path == "/app/") {
		http.Redirect(w, r, "/app/"+a.id+"/", 303)
		return
	}
	if strings.HasPrefix(path, "/app/") {
		g.workspace(w, r, a, token)
		return
	}
	cloudkit.Error(w, 404, "not found")
}
