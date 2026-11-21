package main

import (
	"context"
	"database/sql"
	"encoding/json"
	"net/http"
	"net/http/cookiejar"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/coder/websocket/wsjson"
	"jeeves/internal/cloudkit"
)

func TestPairedMacAcrossGatewayReplicas(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 8*time.Second)
	defer cancel()
	g := testGateway(t)
	var owner, stranger user
	err := g.transaction(ctx, func(tx *sql.Tx) error {
		var err error
		owner, err = g.store.create(ctx, tx, "owner@example.test", "", "", true)
		if err != nil {
			return err
		}
		stranger, err = g.store.create(ctx, tx, "stranger@example.test", "", "", true)
		return err
	})
	if err != nil {
		t.Fatal(err)
	}
	const relayKey = "tenant-distinct-runtime-relay-key"
	control := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var proof struct{ Tenant, Token string }
		if r.URL.Path != "/internal/runtime/authorize" || json.NewDecoder(r.Body).Decode(&proof) != nil ||
			proof.Tenant != owner.id || proof.Token != relayKey || !cloudkit.Authorized(r, "private-service-token") {
			w.WriteHeader(403)
			return
		}
		w.WriteHeader(204)
	}))
	defer control.Close()
	g.controlURL, g.controlToken = control.URL, "private-service-token"
	edge := httptest.NewUnstartedServer(g.handler())
	edge.StartTLS()
	defer edge.Close()
	g.origin, err = url.Parse(edge.URL)
	if err != nil {
		t.Fatal(err)
	}
	replica := httptest.NewServer(g.handler())
	defer replica.Close()
	client := edge.Client()
	client.Jar, _ = cookiejar.New(nil)
	logIn := func(a user) {
		t.Helper()
		token, err := g.store.session(ctx, a, "Test")
		if err != nil {
			t.Fatal(err)
		}
		client.Jar.SetCookies(g.origin, []*http.Cookie{{Name: g.cookieName(), Value: token, Path: "/", Secure: true}})
	}
	request := func(method, endpoint, token, body string, expected int) {
		t.Helper()
		r, err := http.NewRequestWithContext(ctx, method, endpoint, strings.NewReader(body))
		if err != nil {
			t.Fatal(err)
		}
		r.Header.Set("Origin", g.origin.String())
		r.Header.Set("Content-Type", "application/json")
		if token != "" {
			r.Header.Set("Authorization", "Bearer "+token)
		}
		res, err := client.Do(r)
		if err != nil {
			t.Fatal(err)
		}
		defer res.Body.Close()
		if res.StatusCode != expected {
			t.Fatalf("%s %s returned %d, want %d", method, r.URL.Path, res.StatusCode, expected)
		}
	}
	var pairing struct{ Credential, URL string }
	if err := requestJSON(ctx, client, "POST", edge.URL+"/api/mac/register", "", []byte(`{"name":"Test Mac"}`), &pairing); err != nil {
		t.Fatal(err)
	}
	logIn(owner)
	request("GET", pairing.URL, "", "", 200)
	request("POST", edge.URL+"/api/mac/confirm", "", "", 200)
	var macs []struct{ ID, Name string }
	if err := requestJSON(ctx, client, "GET", edge.URL+"/api/mac/devices", "", nil, &macs); err != nil || len(macs) != 1 || macs[0].Name != "Test Mac" {
		t.Fatalf("device list %#v: %v", macs, err)
	}
	logIn(stranger)
	request("DELETE", edge.URL+"/api/mac/devices/"+macs[0].ID, "", "", 404)
	socket, _, err := websocket.Dial(ctx, strings.Replace(edge.URL, "https://", "wss://", 1)+"/api/mac/socket", &websocket.DialOptions{
		HTTPClient: client, HTTPHeader: http.Header{"Authorization": {"Bearer " + pairing.Credential}},
	})
	if err != nil {
		t.Fatal(err)
	}
	defer socket.CloseNow()
	called := make(chan error, 1)
	go func() {
		var cmd deviceRequest
		err := wsjson.Read(ctx, socket, &cmd)
		if err == nil && (cmd.Link != "0123456789abcdef0123456789abcdef" || string(cmd.Command) != `{"kind":"select"}`) {
			t.Error("Mac received the wrong task window request")
		}
		if err == nil {
			err = wsjson.Write(ctx, socket, deviceReply{ID: cmd.ID, Reply: json.RawMessage(`{"kind":"observed"}`)})
		}
		called <- err
	}()
	wire, err := json.Marshal(relayCommand{owner.id, "0123456789abcdef0123456789abcdef", json.RawMessage(`{"kind":"select"}`)})
	if err != nil {
		t.Fatal(err)
	}
	request("POST", replica.URL+"/internal/desktop", "wrong-runtime-credential", string(wire), 401)
	var reply struct{ Kind string }
	if err := requestJSON(ctx, client, "POST", replica.URL+"/internal/desktop", relayKey, wire, &reply); err != nil || reply.Kind != "observed" {
		t.Fatalf("desktop reply %#v: %v", reply, err)
	}
	if err := <-called; err != nil {
		t.Fatal(err)
	}
	logIn(owner)
	request("DELETE", edge.URL+"/api/mac/devices/"+macs[0].ID, "", "", 200)
	request("GET", edge.URL+"/api/mac/status", pairing.Credential, "", 403)
}
