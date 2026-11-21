package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io/fs"
	"net/http"
	"net/http/httputil"
	"path"
	"strings"
	"time"

	"jeeves/internal/cloudkit"
)

func (g *gateway) locate(ctx context.Context, tenant string) (cloudkit.RuntimeEndpoint, error) {
	if g.runtimeHome != "" {
		return g.discover(ctx)
	}
	if g.fixed.URL != "" {
		return g.fixed, nil
	}
	var target cloudkit.RuntimeEndpoint
	err := g.controlTenant(ctx, "/internal/runtime", tenant, &target)
	return target, err
}

func (g *gateway) controlTenant(ctx context.Context, endpoint, tenant string, into any) error {
	body, err := json.Marshal(cloudkit.RuntimeRequest{Tenant: tenant})
	if err != nil {
		return err
	}
	req, err := http.NewRequestWithContext(ctx, "POST", g.controlURL+endpoint, bytes.NewReader(body))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Authorization", "Bearer "+g.controlToken)
	resp, err := g.http.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 {
		return errors.New("control plane unavailable")
	}
	return json.NewDecoder(resp.Body).Decode(into)
}

func (g *gateway) workspace(w http.ResponseWriter, r *http.Request, a user, token string) {
	id, suffix, ok := strings.Cut(strings.TrimPrefix(r.URL.Path, "/app/"), "/")
	if !ok || id != a.id {
		cloudkit.Error(w, 403, "workspace belongs to another account")
		return
	}
	if suffix == "api/workspace" && r.Method == "GET" {
		target, err := g.locate(r.Context(), a.id)
		if err != nil {
			failure(w, err)
			return
		}
		ready := target.Ready
		if g.fixed.URL != "" {
			req, err := http.NewRequestWithContext(r.Context(), http.MethodGet, target.URL+"/healthz", nil)
			if err != nil {
				failure(w, err)
				return
			}
			resp, err := g.http.Do(req)
			ready = err == nil && resp.StatusCode == http.StatusOK
			if resp != nil {
				resp.Body.Close()
			}
		}
		cloudkit.JSON(w, 200, cloudkit.WorkspaceState{Ready: ready, Managed: g.controlURL != ""})
		return
	}
	if !strings.HasPrefix(suffix, "api/") {
		if r.Method != "GET" && r.Method != "HEAD" {
			cloudkit.Error(w, 405, "method not allowed")
			return
		}
		if suffix == "" {
			suffix = "index.html"
		}
		g.asset(w, r, suffix)
		return
	}
	if suffix == "api/gateway/rpc" || !fs.ValidPath(suffix) || strings.Contains(suffix, "\\") {
		cloudkit.Error(w, 404, "not found")
		return
	}
	target, err := g.locate(r.Context(), a.id)
	if err != nil {
		failure(w, err)
		return
	}
	if !target.Ready {
		w.Header().Set("Retry-After", "2")
		cloudkit.Error(w, 503, "workspace is starting")
		return
	}
	upstream, err := cloudkit.Origin(target.URL)
	if err != nil {
		failure(w, err)
		return
	}
	ctx, cancel := g.liveRequest(r.Context(), token)
	defer cancel()
	proxy := httputil.ReverseProxy{
		Rewrite: func(p *httputil.ProxyRequest) {
			p.SetURL(upstream)
			p.Out.URL.Path = "/" + suffix
			p.Out.URL.RawPath = ""
			p.Out.Header.Del("Cookie")
			p.Out.Header.Del("Origin")
			p.Out.Header.Set("Authorization", "Bearer "+target.Token)
		},
		ModifyResponse: func(res *http.Response) error {
			res.Header.Del("Set-Cookie")
			res.Header.Set("Content-Security-Policy", "sandbox; default-src 'none'")
			if strings.HasPrefix(res.Header.Get("Content-Type"), "text/event-stream") {
				res.Header.Set("X-Accel-Buffering", "no")
			}
			return nil
		},
		ErrorHandler: func(w http.ResponseWriter, _ *http.Request, _ error) {
			cloudkit.Error(w, 502, "workspace connection interrupted")
		},
	}
	proxy.ServeHTTP(w, r.WithContext(ctx))
}
func (g *gateway) liveRequest(parent context.Context, token string) (context.Context, context.CancelFunc) {
	ctx, cancel := context.WithCancel(parent)
	stop := func() bool { return false }
	if g.shutdown != nil {
		stop = context.AfterFunc(g.shutdown, cancel)
	}
	go func() {
		ticker := time.NewTicker(2 * time.Second)
		defer ticker.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				check, end := context.WithTimeout(ctx, 2*time.Second)
				_, _, err := g.store.authenticate(check, token)
				end()
				if err != nil {
					cancel()
					return
				}
			}
		}
	}()
	return ctx, func() { stop(); cancel() }
}

func (g *gateway) asset(w http.ResponseWriter, r *http.Request, name string) {
	switch path.Ext(name) {
	case ".html", ".js", ".css", ".png", ".jpg", ".svg", ".webmanifest", ".woff2", ".ico":
	default:
		cloudkit.Error(w, 404, "not found")
		return
	}
	if !fs.ValidPath(name) || strings.Contains(name, "\\") {
		cloudkit.Error(w, 404, "not found")
		return
	}
	w.Header().Set("Content-Security-Policy", "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'; frame-ancestors 'none'; object-src 'none'; base-uri 'none'; form-action 'self'")
	file, err := http.Dir(g.bundle).Open("/" + name)
	if err != nil {
		cloudkit.Error(w, 404, "asset unavailable")
		return
	}
	defer file.Close()
	info, err := file.Stat()
	if err != nil || !info.Mode().IsRegular() {
		cloudkit.Error(w, 404, "not found")
		return
	}
	http.ServeContent(w, r, name, info.ModTime(), file)
}
