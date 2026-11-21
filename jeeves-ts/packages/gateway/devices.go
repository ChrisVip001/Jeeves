package main

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"html/template"
	"net/http"
	"strings"
	"time"

	"github.com/coder/websocket"
	"github.com/coder/websocket/wsjson"
	"jeeves/internal/cloudkit"
)

func init() {
	schema = append(schema,
		`CREATE TABLE IF NOT EXISTS gw_macs (
			id TEXT PRIMARY KEY, proof TEXT NOT NULL UNIQUE, claim TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
			owner TEXT REFERENCES gw_users(id) ON DELETE CASCADE, generation TEXT NOT NULL DEFAULT '',
			expires BIGINT NOT NULL, seen BIGINT NOT NULL DEFAULT 0)`,
		`CREATE TABLE IF NOT EXISTS gw_mac_links (
			link TEXT PRIMARY KEY, owner TEXT NOT NULL, device TEXT NOT NULL REFERENCES gw_macs(id) ON DELETE CASCADE,
			generation TEXT NOT NULL, expires BIGINT NOT NULL)`,
		`CREATE TABLE IF NOT EXISTS gw_mac_operations (
			id TEXT PRIMARY KEY, link TEXT NOT NULL REFERENCES gw_mac_links(link) ON DELETE CASCADE,
			device TEXT NOT NULL, generation TEXT NOT NULL, input TEXT NOT NULL, output TEXT NOT NULL DEFAULT '',
			state TEXT NOT NULL DEFAULT 'waiting', expires BIGINT NOT NULL)`,
		`CREATE INDEX IF NOT EXISTS gw_mac_inbox ON gw_mac_operations(device,generation,state)`,
	)
}

type mac struct {
	ID, Name, Generation string
	Seen                 int64
}
type desktopCommand struct {
	Kind          string  `json:"kind"`
	ObservationID string  `json:"observationId"`
	ElementID     int     `json:"elementId"`
	Action        string  `json:"action"`
	Text          *string `json:"text"`
}
type relayCommand struct {
	Tenant  string          `json:"tenant"`
	Link    string          `json:"link"`
	Command json.RawMessage `json:"command"`
}
type deviceRequest struct {
	ID      string          `json:"id"`
	Link    string          `json:"link"`
	Command json.RawMessage `json:"command"`
}
type deviceReply struct {
	ID    string          `json:"id"`
	Reply json.RawMessage `json:"reply"`
}

func desktopInput(wire json.RawMessage) (desktopCommand, bool) {
	var command desktopCommand
	if json.Unmarshal(wire, &command) != nil {
		return command, false
	}
	switch command.Kind {
	case "select", "observe":
		return command, true
	case "frame":
		return command, command.ObservationID != "" && len(command.ObservationID) <= 128
	case "act":
		if command.ObservationID == "" || len(command.ObservationID) > 128 || command.ElementID < 0 {
			return command, false
		}
		switch command.Action {
		case "click", "scrollUp", "scrollDown":
			return command, command.Text == nil
		case "setText":
			return command, command.Text != nil && len(*command.Text) <= 4096
		}
	}
	return command, false
}

func (g *gateway) deviceRoute(w http.ResponseWriter, r *http.Request) bool {
	if g.local {
		return false
	}
	switch {
	case r.URL.Path == "/api/mac/register" && r.Method == "POST":
		g.beginPair(w, r)
	case r.URL.Path == "/api/mac/status" && r.Method == "GET":
		g.pairStatus(w, r)
	case r.URL.Path == "/api/mac/socket" && r.Method == "GET":
		g.macSocket(w, r)
	case r.URL.Path == "/device/" && r.Method == "GET":
		g.pairPage(w, r)
	case r.URL.Path == "/api/mac/confirm" && r.Method == "POST":
		g.confirmPair(w, r)
	default:
		return false
	}
	return true
}
func (g *gateway) beginPair(w http.ResponseWriter, r *http.Request) {
	if !g.rate(w, r, "device:"+g.clientIP(r), 12) {
		return
	}
	var fields struct{ Name string }
	if cloudkit.Decode(w, r, 2048, &fields) != nil || len(fields.Name) > 120 || strings.TrimSpace(fields.Name) == "" {
		cloudkit.Error(w, 400, "choose a device name")
		return
	}
	id, proof, claim := cloudkit.Token(16), cloudkit.Token(32), cloudkit.Token(24)
	_, err := g.store.db.ExecContext(r.Context(), `INSERT INTO gw_macs(id,proof,claim,name,expires) VALUES($1,$2,$3,$4,$5)`,
		id, g.store.digest(proof), g.store.digest(claim), fields.Name, time.Now().Add(10*time.Minute).UnixMilli())
	if err != nil {
		failure(w, err)
		return
	}
	cloudkit.JSON(w, 201, map[string]string{"credential": proof, "url": g.origin.String() + "/device/?code=" + claim})
}
func (g *gateway) pairStatus(w http.ResponseWriter, r *http.Request) {
	proof := cloudkit.Bearer(r)
	var paired bool
	err := g.store.db.QueryRowContext(r.Context(), `SELECT owner IS NOT NULL FROM gw_macs WHERE proof=$1 AND (owner IS NOT NULL OR expires>$2)`,
		g.store.digest(proof), time.Now().UnixMilli()).Scan(&paired)
	if err != nil {
		failure(w, err)
		return
	}
	cloudkit.JSON(w, 200, map[string]bool{"paired": paired})
}

const pairCookie = "__Host-jeeves_pair"

var pairHTML = template.Must(template.New("device").Parse(`<!doctype html><html lang="zh"><meta charset="utf-8"><meta name="viewport" content="width=device-width">
<title>Jeeves · 连接 Mac</title><main><h1>连接这台 Mac / Connect this Mac</h1><p>{{.}}</p>
<p>连接后，Jeeves 可以请求你为任务选择窗口。 / Jeeves can request a window for a task after pairing.</p>
<form method="post" action="/api/mac/confirm"><button>连接 / Connect</button></form></main></html>`))

func (g *gateway) pairPage(w http.ResponseWriter, r *http.Request) {
	code := r.URL.Query().Get("code")
	if code != "" {
		if len(code) != 48 {
			failure(w, rejected)
			return
		}
		http.SetCookie(w, &http.Cookie{Name: pairCookie, Value: code, Path: "/", HttpOnly: true, Secure: true, SameSite: http.SameSiteLaxMode, MaxAge: 600})
		http.Redirect(w, r, "/device/", 303)
		return
	}
	_, _, err := g.store.authenticate(r.Context(), g.sessionToken(r))
	if err != nil {
		http.Redirect(w, r, "/account/?next=%2Fdevice%2F", 303)
		return
	}
	cookie, err := r.Cookie(pairCookie)
	if err != nil {
		failure(w, rejected)
		return
	}
	var name string
	err = g.store.db.QueryRowContext(r.Context(), "SELECT name FROM gw_macs WHERE claim=$1 AND expires>$2 AND owner IS NULL",
		g.store.digest(cookie.Value), time.Now().UnixMilli()).Scan(&name)
	if err != nil {
		failure(w, err)
		return
	}
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.Header().Set("Content-Security-Policy", "default-src 'none'; form-action 'self'; frame-ancestors 'none'")
	if err := pairHTML.Execute(w, name); err != nil {
		return
	}
}
func (g *gateway) confirmPair(w http.ResponseWriter, r *http.Request) {
	if r.Header.Get("Origin") != g.origin.String() {
		failure(w, rejected)
		return
	}
	_, u, err := g.store.authenticate(r.Context(), g.sessionToken(r))
	if err != nil {
		failure(w, err)
		return
	}
	c, err := r.Cookie(pairCookie)
	if err != nil {
		failure(w, rejected)
		return
	}
	result, err := g.store.db.ExecContext(r.Context(), `UPDATE gw_macs SET owner=$1 WHERE claim=$2 AND expires>$3 AND owner IS NULL`,
		u.id, g.store.digest(c.Value), time.Now().UnixMilli())
	if err != nil {
		failure(w, err)
		return
	}
	if err := one(result); err != nil {
		failure(w, err)
		return
	}
	http.SetCookie(w, &http.Cookie{Name: pairCookie, Value: "", Path: "/", Secure: true, HttpOnly: true, MaxAge: -1})
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	w.Write([]byte("Mac 已连接。你可以回到 Jeeves。\nYour Mac is connected. You can return to Jeeves."))
}
func (g *gateway) devices(w http.ResponseWriter, r *http.Request, u user) {
	ctx := r.Context()
	if r.Method == "DELETE" {
		id := strings.TrimPrefix(r.URL.Path, "/api/mac/devices/")
		if !sessionID.MatchString(id) {
			cloudkit.Error(w, 404, "device not found")
			return
		}
		result, err := g.store.db.ExecContext(ctx, "DELETE FROM gw_macs WHERE id=$1 AND owner=$2", id, u.id)
		if err != nil {
			failure(w, err)
			return
		}
		if one(result) != nil {
			cloudkit.Error(w, 404, "device not found")
			return
		}
		cloudkit.JSON(w, 200, map[string]bool{"ok": true})
		return
	}
	rows, err := g.store.db.QueryContext(ctx, "SELECT id,name,seen FROM gw_macs WHERE owner=$1 ORDER BY seen DESC", u.id)
	if err != nil {
		failure(w, err)
		return
	}
	defer rows.Close()
	list := []map[string]any{}
	for rows.Next() {
		var a mac
		if err := rows.Scan(&a.ID, &a.Name, &a.Seen); err != nil {
			failure(w, err)
			return
		}
		list = append(list, map[string]any{"id": a.ID, "name": a.Name, "online": a.Seen > time.Now().Add(-9*time.Second).UnixMilli()})
	}
	if err := rows.Err(); err != nil {
		failure(w, err)
		return
	}
	cloudkit.JSON(w, 200, list)
}

func (g *gateway) macSocket(w http.ResponseWriter, r *http.Request) {
	ctx, cancel := context.WithCancel(r.Context())
	defer cancel()
	if g.shutdown != nil {
		defer context.AfterFunc(g.shutdown, cancel)()
	}
	var id string
	generation := cloudkit.Token(12)
	err := g.store.db.QueryRowContext(ctx, `UPDATE gw_macs SET generation=$1,seen=$2 WHERE proof=$3 AND owner IS NOT NULL RETURNING id`,
		generation, time.Now().UnixMilli(), g.store.digest(cloudkit.Bearer(r))).Scan(&id)
	if err != nil {
		failure(w, err)
		return
	}
	conn, err := websocket.Accept(w, r, nil)
	if err != nil {
		return
	}
	conn.SetReadLimit(1600000)
	read := make(chan deviceReply, 1)
	go func() {
		defer cancel()
		for {
			var reply deviceReply
			if err := wsjson.Read(ctx, conn, &reply); err != nil {
				return
			}
			select {
			case read <- reply:
			case <-ctx.Done():
				return
			}
		}
	}()
	defer conn.CloseNow()
	defer func() {
		end, stop := context.WithTimeout(context.Background(), 2*time.Second)
		defer stop()
		g.store.db.ExecContext(end, "UPDATE gw_macs SET seen=0 WHERE id=$1 AND generation=$2", id, generation)
	}()
	tick := time.NewTicker(500 * time.Millisecond)
	defer tick.Stop()
	beats := 0
	for {
		select {
		case <-ctx.Done():
			return
		case <-tick.C:
		}
		if beats%6 == 0 {
			result, err := g.store.db.ExecContext(ctx, "UPDATE gw_macs SET seen=$1 WHERE id=$2 AND generation=$3", time.Now().UnixMilli(), id, generation)
			if err != nil || one(result) != nil {
				return
			}
		}
		beats++
		var cmd deviceRequest
		var command string
		err := g.store.db.QueryRowContext(ctx, `UPDATE gw_mac_operations SET state='running'
		WHERE id=(SELECT id FROM gw_mac_operations WHERE device=$1 AND generation=$2 AND state='waiting' AND expires>$3 ORDER BY expires LIMIT 1)
		AND state='waiting' RETURNING id,link,input`, id, generation, time.Now().UnixMilli()).Scan(&cmd.ID, &cmd.Link, &command)
		if errors.Is(err, sql.ErrNoRows) {
			continue
		}
		if err != nil {
			return
		}
		clear, err := g.store.box.Open(command, cmd.ID+":input")
		if err != nil {
			return
		}
		cmd.Command = json.RawMessage(clear)
		if !g.exchangeDevice(ctx, conn, read, id, generation, cmd) {
			return
		}
	}
}

func (g *gateway) exchangeDevice(ctx context.Context, conn *websocket.Conn, read <-chan deviceReply, id, generation string, cmd deviceRequest) bool {
	write, end := context.WithTimeout(ctx, 3*time.Second)
	err := wsjson.Write(write, conn, cmd)
	end()
	if err != nil {
		return false
	}
	tick := time.NewTicker(time.Second)
	defer tick.Stop()
	for {
		select {
		case <-ctx.Done():
			return false
		case reply := <-read:
			if reply.ID != cmd.ID || !validReply(reply.Reply) {
				return false
			}
			_, err := g.store.db.ExecContext(ctx, `UPDATE gw_mac_operations SET output=$1,state='done' WHERE id=$2 AND state='running'`, g.store.box.Seal(string(reply.Reply), cmd.ID+":output"), cmd.ID)
			return err == nil
		case <-tick.C:
			result, err := g.store.db.ExecContext(ctx, `UPDATE gw_macs SET seen=$1 WHERE id=$2 AND generation=$3
			AND EXISTS(SELECT 1 FROM gw_mac_operations WHERE id=$4 AND state='running' AND expires>$1)`,
				time.Now().UnixMilli(), id, generation, cmd.ID)
			if err != nil || one(result) != nil {
				return false
			}
		}
	}
}
