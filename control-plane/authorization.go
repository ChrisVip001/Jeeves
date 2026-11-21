package main

import (
	"crypto/hmac"
	"net/http"

	"jeeves/internal/cloudkit"
)

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
