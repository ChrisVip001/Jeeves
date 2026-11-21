package main

import (
	"context"
	"errors"
	"fmt"
	"log"
	"net/http"
	"net/url"
	"os"
	"strconv"
	"strings"
	"sync"
	"time"

	"jeeves/internal/cloudkit"
	"k8s.io/client-go/dynamic"
	"k8s.io/client-go/rest"
)

type plane struct {
	registry
	cluster                                cluster
	gatewayKey, namespace, image, modelURL string
	provider                               *url.URL
	desktopKey                             []byte
	providerKey                            string
	quota                                  int64
	idle                                   time.Duration
	probe, modelHTTP                       *http.Client
	fixed                                  cloudkit.RuntimeEndpoint
	fixedTenant                            string
}
type cluster interface {
	ensure(ctx context.Context, t tenant) error
	scale(ctx context.Context, id string, replicas int64) error
	url(id string) string
}

func main() {
	ctx, end := context.WithTimeout(context.Background(), 15*time.Second)
	p, err := configure(ctx)
	end()
	if err != nil {
		log.Fatal(err)
	}
	defer p.db.Close()
	if err := cloudkit.Serve(cloudkit.Env("CP_ADDR", ":8080"), os.Getenv("CP_TLS_CERT"), os.Getenv("CP_TLS_KEY"), func(ctx context.Context) http.Handler {
		if p.cluster != nil {
			go p.worker(ctx)
		}
		return p.handler()
	}); err != nil {
		log.Fatal(err)
	}
}
func configure(ctx context.Context) (*plane, error) {
	dsn, key, auth := os.Getenv("CP_DATABASE_URL"), os.Getenv("CP_STATE_KEY"), os.Getenv("CP_GATEWAY_TOKEN")
	if dsn == "" || len(key) < 32 || len(auth) < 32 {
		return nil, errors.New("control plane requires database and independent storage and service credentials")
	}
	dev := os.Getenv("CP_DEVELOPMENT") == "1"
	if strings.HasPrefix(dsn, "sqlite:") && !dev {
		return nil, errors.New("use PostgreSQL for the cloud control plane")
	}
	db, err := cloudkit.Open(ctx, dsn)
	if err != nil {
		return nil, err
	}
	box, err := cloudkit.Encryption(cloudkit.MAC([]byte(key), "jeeves-control-plane"))
	if err != nil {
		return nil, err
	}
	if err := cloudkit.Schema(ctx, db, schema); err != nil {
		db.Close()
		return nil, err
	}
	quota, err := strconv.ParseInt(cloudkit.Env("CP_MONTHLY_TOKENS", "2000000"), 10, 64)
	if err != nil || quota <= 0 {
		return nil, errors.New("invalid model quota")
	}
	idle, err := time.ParseDuration(cloudkit.Env("CP_IDLE", "20m"))
	if err != nil || idle < time.Minute {
		return nil, errors.New("idle interval must be at least one minute")
	}
	rejectRedirect := func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }
	p := &plane{registry: registry{db, box}, gatewayKey: auth, quota: quota, idle: idle, desktopKey: cloudkit.MAC([]byte(key), "jeeves-desktop-v1"),
		probe:     &http.Client{Timeout: 3 * time.Second, CheckRedirect: rejectRedirect},
		modelHTTP: &http.Client{CheckRedirect: rejectRedirect}}
	if fixed := os.Getenv("CP_TEST_RUNTIME_URL"); dev && fixed != "" {
		u, err := cloudkit.Origin(fixed)
		p.fixedTenant = os.Getenv("CP_TEST_TENANT")
		if err != nil || (p.fixedTenant != "" && !cloudkit.TenantPattern.MatchString(p.fixedTenant)) {
			return nil, errors.New("fixed developer Runtime requires a tenant")
		}
		if _, err := db.ExecContext(ctx, "CREATE TABLE IF NOT EXISTS cp_development_binding (slot INTEGER PRIMARY KEY, owner TEXT NOT NULL)"); err != nil {
			return nil, err
		}
		p.fixed = cloudkit.RuntimeEndpoint{URL: u.String(), Token: os.Getenv("CP_TEST_RUNTIME_TOKEN"), Ready: true}
		return p, nil
	}
	p.image, p.namespace = os.Getenv("CP_IMAGE"), cloudkit.Env("CP_NAMESPACE", "jeeves")
	p.modelURL, p.providerKey = os.Getenv("CP_MODEL_URL"), os.Getenv("CP_MODEL_KEY")
	p.provider, err = cloudkit.Origin(os.Getenv("CP_MODEL_UPSTREAM"))
	if err != nil || p.provider.Scheme != "https" || p.image == "" || p.modelURL == "" || p.providerKey == "" {
		return nil, errors.New("configure Runtime image, model proxy origin and HTTPS model provider")
	}
	model, err := cloudkit.Origin(p.modelURL)
	if err != nil {
		return nil, err
	}
	p.modelURL = model.String()
	kube, err := rest.InClusterConfig()
	if err != nil {
		return nil, fmt.Errorf("load Kubernetes credentials: %w", err)
	}
	kube.Timeout = 5 * time.Second
	client, err := dynamic.NewForConfig(kube)
	if err != nil {
		return nil, err
	}
	p.cluster = kubernetes{client: client, namespace: p.namespace, image: p.image, modelURL: p.modelURL, desktopKey: p.desktopKey, gatewayURL: cloudkit.Env("CP_DESKTOP_GATEWAY_URL", "http://gateway.jeeves.svc:8080")}
	return p, nil
}
func (p *plane) handler() http.Handler {
	router := http.NewServeMux()
	router.HandleFunc("GET /healthz", func(w http.ResponseWriter, r *http.Request) {
		if err := p.db.PingContext(r.Context()); err != nil {
			cloudkit.Error(w, 503, "database unavailable")
			return
		}
		cloudkit.JSON(w, 200, map[string]bool{"ok": true})
	})
	router.HandleFunc("POST /internal/runtime", p.runtime)
	router.HandleFunc("POST /internal/runtime/authorize", p.authorizeRuntime)
	router.HandleFunc("POST /internal/usage", p.usage)
	router.HandleFunc("POST /v1/messages", p.model)
	return router
}
func (p *plane) runtime(w http.ResponseWriter, r *http.Request) {
	if !cloudkit.Authorized(r, p.gatewayKey) {
		cloudkit.Error(w, 401, "gateway authentication required")
		return
	}
	var req cloudkit.RuntimeRequest
	if cloudkit.Decode(w, r, 2048, &req) != nil || !cloudkit.TenantPattern.MatchString(req.Tenant) {
		cloudkit.Error(w, 400, "invalid tenant")
		return
	}
	if p.fixed.URL != "" {
		owner := p.fixedTenant
		ctx := r.Context()
		if owner == "" {
			if _, err := p.db.ExecContext(ctx, "INSERT INTO cp_development_binding(slot,owner) VALUES(1,$1) ON CONFLICT(slot) DO NOTHING", req.Tenant); err != nil {
				internalError(w, err)
				return
			}
			if err := p.db.QueryRowContext(ctx, "SELECT owner FROM cp_development_binding WHERE slot=1").Scan(&owner); err != nil {
				internalError(w, err)
				return
			}
		}
		if req.Tenant != owner {
			cloudkit.Error(w, 404, "workspace unavailable")
			return
		}
		cloudkit.JSON(w, 200, p.fixed)
		return
	}
	t, err := p.wake(r.Context(), req.Tenant)
	if err != nil {
		internalError(w, err)
		return
	}
	cloudkit.JSON(w, 200, cloudkit.RuntimeEndpoint{URL: p.cluster.url(t.id), Token: t.ops, Ready: t.ready && t.wanted})
}
func internalError(w http.ResponseWriter, err error) {
	if errors.Is(err, context.Canceled) {
		return
	}
	log.Printf("control-plane request failed (%T)", err)
	cloudkit.Error(w, 503, "control plane unavailable")
}
func (p *plane) worker(ctx context.Context) {
	tick := time.NewTicker(time.Second)
	defer tick.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-tick.C:
		}
		ids, err := p.due(ctx)
		if err != nil {
			log.Printf("read Runtime work (%T)", err)
			continue
		}
		var wg sync.WaitGroup
		for _, id := range ids {
			wg.Go(func() {
				work, cancel := context.WithTimeout(ctx, 20*time.Second)
				defer cancel()
				t, err := p.claim(work, id)
				if err != nil {
					log.Printf("claim Runtime %s (%T)", id, err)
					return
				}
				if t.id != "" {
					p.reconcile(work, t)
				}
			})
		}
		wg.Wait()
	}
}
