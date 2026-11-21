package main

import (
	"bytes"
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"net/http"
	"time"

	"jeeves/internal/cloudkit"
)

func (g *gateway) relay(w http.ResponseWriter, r *http.Request) {
	var request relayCommand
	if cloudkit.Decode(w, r, 16<<10, &request) != nil ||
		!cloudkit.TenantPattern.MatchString(request.Tenant) || !sessionID.MatchString(request.Link) {
		cloudkit.Error(w, 400, "invalid desktop command")
		return
	}
	if !g.runtimeCredential(r.Context(), request.Tenant, cloudkit.Bearer(r)) {
		cloudkit.Error(w, 401, "Runtime authentication required")
		return
	}
	var err error
	command, ok := desktopInput(request.Command)
	if !ok {
		cloudkit.Error(w, 400, "invalid desktop command")
		return
	}
	var device mac
	ctx := r.Context()
	if command.Kind == "select" {
		err = g.store.db.QueryRowContext(ctx, "SELECT id,generation FROM gw_macs WHERE owner=$1 AND seen>$2 ORDER BY seen DESC LIMIT 1",
			request.Tenant, time.Now().Add(-9*time.Second).UnixMilli()).Scan(&device.ID, &device.Generation)
		if err == nil {
			_, err = g.store.db.ExecContext(ctx, `INSERT INTO gw_mac_links(link,owner,device,generation,expires) VALUES($1,$2,$3,$4,$5)`,
				request.Link, request.Tenant, device.ID, device.Generation, time.Now().Add(10*time.Minute).UnixMilli())
		}
	} else {
		err = g.store.db.QueryRowContext(ctx, `SELECT m.id,m.generation FROM gw_mac_links l JOIN gw_macs m ON m.id=l.device AND m.generation=l.generation
			WHERE l.link=$1 AND l.owner=$2 AND m.owner=$2 AND l.expires>$3 AND m.seen>$4`,
			request.Link, request.Tenant, time.Now().UnixMilli(), time.Now().Add(-9*time.Second).UnixMilli()).Scan(&device.ID, &device.Generation)
	}
	if errors.Is(err, sql.ErrNoRows) {
		cloudkit.JSON(w, 200, map[string]string{"kind": "reselect", "message": "Connect your Mac and choose a window for this task."})
		return
	}
	if err != nil {
		failure(w, err)
		return
	}
	timeout := 12 * time.Second
	if command.Kind == "select" {
		timeout = 80 * time.Second
	}
	id := cloudkit.Token(16)
	_, err = g.store.db.ExecContext(ctx, `INSERT INTO gw_mac_operations(id,link,device,generation,input,expires) VALUES($1,$2,$3,$4,$5,$6)`,
		id, request.Link, device.ID, device.Generation, g.store.box.Seal(string(request.Command), id+":input"), time.Now().Add(timeout).UnixMilli())
	if err != nil {
		failure(w, err)
		return
	}
	defer func() {
		end, stop := context.WithTimeout(context.Background(), 2*time.Second)
		defer stop()
		g.store.db.ExecContext(end, `DELETE FROM gw_mac_operations WHERE id=$1`, id)
	}()
	timed, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	tick := time.NewTicker(250 * time.Millisecond)
	defer tick.Stop()
	for {
		select {
		case <-timed.Done():
			cloudkit.JSON(w, 200, map[string]string{"kind": "unknown", "message": "The Mac did not confirm the result. Check the selected window before retrying."})
			return
		case <-tick.C:
		}
		var state, output string
		err := g.store.db.QueryRowContext(timed, `SELECT o.state,o.output FROM gw_mac_operations o JOIN gw_macs m
			ON m.id=o.device AND m.generation=o.generation WHERE o.id=$1 AND m.seen>$2`,
			id, time.Now().Add(-9*time.Second).UnixMilli()).Scan(&state, &output)
		if errors.Is(err, sql.ErrNoRows) {
			cloudkit.JSON(w, 200, map[string]string{"kind": "unknown", "message": "The Mac disconnected. Check whether the action completed."})
			return
		}
		if err != nil {
			failure(w, err)
			return
		}
		if state == "done" {
			clear, err := g.store.box.Open(output, id+":output")
			if err != nil {
				failure(w, err)
				return
			}
			cloudkit.JSON(w, 200, json.RawMessage(clear))
			return
		}
	}
}

func (g *gateway) runtimeCredential(ctx context.Context, tenant, token string) bool {
	if g.controlURL == "" || len(token) < 16 || len(token) > 128 {
		return false
	}
	input, err := json.Marshal(map[string]string{"tenant": tenant, "token": token})
	if err != nil {
		return false
	}
	req, err := http.NewRequestWithContext(ctx, "POST", g.controlURL+"/internal/runtime/authorize", bytes.NewReader(input))
	if err != nil {
		return false
	}
	req.Header.Set("Authorization", "Bearer "+g.controlToken)
	req.Header.Set("Content-Type", "application/json")
	res, err := g.http.Do(req)
	if err != nil {
		return false
	}
	res.Body.Close()
	return res.StatusCode == 204
}
