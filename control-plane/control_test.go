package main

import (
	"context"
	"fmt"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"jeeves/internal/cloudkit"
)

func testPlane(t *testing.T) *plane {
	t.Helper()
	ctx := context.Background()
	dsn := "sqlite:" + filepath.Join(t.TempDir(), "control.sqlite")
	db, err := cloudkit.Open(ctx, dsn)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { db.Close() })
	if err := cloudkit.Schema(ctx, db, schema); err != nil {
		t.Fatal(err)
	}
	box, err := cloudkit.Encryption(make([]byte, 32))
	if err != nil {
		t.Fatal(err)
	}
	return &plane{registry: registry{db: db, box: box}, quota: 10, probe: &http.Client{Timeout: time.Second}, idle: time.Hour}
}
func TestConcurrentReservationsNeverSpendTheSameBalance(t *testing.T) {
	ctx := context.Background()
	p := testPlane(t)
	tenant, err := p.wake(ctx, "u-000000000001")
	if err != nil {
		t.Fatal(err)
	}
	var wg sync.WaitGroup
	var admitted atomic.Int64
	var winner atomic.Value
	for i := range 8 {
		wg.Go(func() {
			id := fmt.Sprintf("fixture-%d", i)
			err := p.reserve(ctx, id, tenant.id, "2026-11", 6)
			switch err {
			case nil:
				admitted.Add(1)
				winner.Store(id)
			case limitReached:
			default:
				t.Errorf("reserve: %v", err)
			}
		})
	}
	wg.Wait()
	if got := admitted.Load(); got != 1 {
		t.Fatalf("admitted %d reservations", got)
	}
	for range 2 {
		if err := p.finalize(ctx, winner.Load().(string), 4); err != nil {
			t.Fatal(err)
		}
	}
	var used, reserved int
	if err := p.db.QueryRowContext(ctx, "SELECT used,reserved FROM cp_usage WHERE tenant=$1", tenant.id).Scan(&used, &reserved); err != nil {
		t.Fatal(err)
	}
	if used != 4 || reserved != 0 {
		t.Fatalf("used=%d reserved=%d", used, reserved)
	}
}

type testCluster struct {
	address string
	wakes   atomic.Int64
	sleeps  atomic.Int64
}

func (k *testCluster) url(string) string                          { return k.address }
func (k *testCluster) ensure(context.Context, tenant) error       { k.wakes.Add(1); return nil }
func (k *testCluster) scale(context.Context, string, int64) error { k.sleeps.Add(1); return nil }

func TestLeaseAndRuntimeHealthDriveProvisioning(t *testing.T) {
	ctx := context.Background()
	p := testPlane(t)
	var ready atomic.Bool
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		if !ready.Load() {
			w.WriteHeader(503)
		}
	}))
	defer srv.Close()
	c := &testCluster{address: srv.URL}
	p.cluster = c
	owner := "u-000000000002"
	if _, err := p.wake(ctx, owner); err != nil {
		t.Fatal(err)
	}
	leased, err := p.claim(ctx, owner)
	if err != nil || leased.id == "" {
		t.Fatalf("claim: %v", err)
	}
	other, err := p.claim(ctx, owner)
	if err != nil || other.id != "" {
		t.Fatalf("second worker claimed %q (%v)", other.id, err)
	}
	p.reconcile(ctx, leased)
	if c.wakes.Load() != 1 {
		t.Fatal("did not provision a Runtime")
	}
	touched, err := p.wake(ctx, owner)
	if err != nil || touched.ready {
		t.Fatal("reported readiness before upstream health")
	}
	ready.Store(true)
	if _, err := p.db.ExecContext(ctx, "UPDATE cp_tenants SET next_check=0 WHERE id=$1", owner); err != nil {
		t.Fatal(err)
	}
	leased, err = p.claim(ctx, owner)
	if err != nil || leased.id == "" {
		t.Fatalf("claim: %v", err)
	}
	p.reconcile(ctx, leased)
	touched, err = p.wake(ctx, owner)
	if err != nil || !touched.ready || touched.ops != leased.ops {
		t.Fatal("did not route back to the same ready Runtime")
	}
}
