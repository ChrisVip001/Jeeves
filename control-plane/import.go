package main

import (
	"crypto/hmac"
	"net/http"
	"regexp"

	"jeeves/internal/cloudkit"
)

var monthPattern = regexp.MustCompile(`^[0-9]{4}-(0[1-9]|1[0-2])$`)

func (p *plane) importTenant(w http.ResponseWriter, r *http.Request) {
	if !cloudkit.Authorized(r, p.gatewayKey) {
		cloudkit.Error(w, 401, "gateway authentication required")
		return
	}
	var req struct {
		Tenant, Ops, Model string
		Usage              map[string]int64
		AlwaysOn           bool
	}
	if cloudkit.Decode(w, r, 48<<10, &req) != nil || !cloudkit.TenantPattern.MatchString(req.Tenant) ||
		len(req.Ops) < 16 || len(req.Ops) > 128 || len(req.Model) < 16 || len(req.Model) > 128 {
		cloudkit.Error(w, 400, "invalid imported tenant")
		return
	}
	for month, used := range req.Usage {
		if !monthPattern.MatchString(month) || used < 0 {
			cloudkit.Error(w, 400, "invalid imported usage")
			return
		}
	}
	ctx := r.Context()
	if err := p.save(ctx, req.Tenant, req.Ops, req.Model, false); err != nil {
		internalError(w, err)
		return
	}
	t, err := p.open(ctx, req.Tenant)
	if err != nil {
		internalError(w, err)
		return
	}
	if t.ops != req.Ops || t.model != req.Model {
		cloudkit.Error(w, 409, "tenant already has different resources")
		return
	}
	tx, err := p.db.BeginTx(ctx, nil)
	if err != nil {
		internalError(w, err)
		return
	}
	defer tx.Rollback()
	if req.AlwaysOn {
		if _, err := tx.ExecContext(ctx, `UPDATE cp_tenants SET always_on=TRUE,wanted=TRUE,next_check=0 WHERE id=$1`, req.Tenant); err != nil {
			internalError(w, err)
			return
		}
	}
	for month, used := range req.Usage {
		if _, err := tx.ExecContext(ctx, `INSERT INTO cp_usage(tenant,period,used) VALUES($1,$2,$3)
		ON CONFLICT(tenant,period) DO NOTHING`, req.Tenant, month, used); err != nil {
			internalError(w, err)
			return
		}
	}
	if err := tx.Commit(); err != nil {
		internalError(w, err)
		return
	}
	cloudkit.JSON(w, 200, map[string]bool{"ok": true})
}

func (p *plane) authorizeRuntime(w http.ResponseWriter, r *http.Request) {
	if !cloudkit.Authorized(r, p.gatewayKey) {
		cloudkit.Error(w, 401, "gateway authentication required")
		return
	}
	var req struct{ Tenant, Token string }
	if cloudkit.Decode(w, r, 2048, &req) != nil || !cloudkit.TenantPattern.MatchString(req.Tenant) || len(req.Token) < 16 || len(req.Token) > 128 {
		cloudkit.Error(w, 400, "invalid Runtime credential")
		return
	}
	if p.fixed.URL != "" {
		var owner string
		err := p.db.QueryRowContext(r.Context(), "SELECT owner FROM cp_development_binding WHERE slot=1").Scan(&owner)
		if err == nil && req.Tenant == owner && secretEqual(req.Token, p.fixed.Token) {
			w.WriteHeader(204)
			return
		}
		cloudkit.Error(w, 403, "unknown Runtime")
		return
	}
	_, err := p.open(r.Context(), req.Tenant)
	if err != nil || !secretEqual(req.Token, relayToken(p.desktopKey, req.Tenant)) {
		cloudkit.Error(w, 403, "unknown Runtime")
		return
	}
	w.WriteHeader(204)
}
func secretEqual(a, b string) bool {
	return hmac.Equal([]byte(cloudkit.Digest(a)), []byte(cloudkit.Digest(b)))
}
