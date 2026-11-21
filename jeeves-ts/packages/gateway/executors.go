package main

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"runtime"
	"strings"
	"syscall"
	"time"

	"github.com/coder/websocket"
	"github.com/coder/websocket/wsjson"
	"jeeves/internal/cloudkit"
)

func executorCLI() bool {
	if filepath.Base(os.Args[0]) == "jeeves-desktop-relay" {
		if err := runRelay(); err != nil {
			log.Fatal(err)
		}
		return true
	}
	if len(os.Args) > 1 && os.Args[1] == "mac-device" {
		if len(os.Args) != 4 || runtime.GOOS != "darwin" {
			log.Fatal("mac-device requires a macOS gateway origin and window helper")
		}
		origin, err := cloudkit.Origin(os.Args[2])
		if err != nil || origin.Scheme != "https" || !filepath.IsAbs(os.Args[3]) {
			log.Fatal("invalid Mac executor settings")
		}
		ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
		defer stop()
		if err := runMac(ctx, origin, os.Args[3]); err != nil && ctx.Err() == nil {
			log.Fatal(err)
		}
		return true
	}
	return false
}

func runRelay() error {
	read := func(name string) string {
		wire, err := os.ReadFile("/var/run/jeeves-relay/" + name)
		if err != nil {
			return ""
		}
		return strings.TrimSpace(string(wire))
	}
	tenant, token, endpoint := read("tenant"), read("token"), read("gateway")
	base, err := cloudkit.Origin(endpoint)
	if err != nil || !cloudkit.TenantPattern.MatchString(tenant) || len(token) < 16 {
		return errors.New("desktop relay is not configured")
	}
	client := &http.Client{Timeout: 85 * time.Second, CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}
	link := cloudkit.Token(16)
	lines := bufio.NewScanner(os.Stdin)
	lines.Buffer(make([]byte, 4096), 16<<10)
	out := json.NewEncoder(os.Stdout)
	for lines.Scan() {
		command := json.RawMessage(lines.Bytes())
		if _, ok := desktopInput(command); !ok {
			return errors.New("invalid window helper command")
		}
		data, err := json.Marshal(relayCommand{tenant, link, command})
		if err != nil {
			return err
		}
		var reply json.RawMessage
		err = requestJSON(context.Background(), client, "POST", base.String()+"/internal/desktop", token, data, &reply)
		if err != nil {
			return out.Encode(map[string]string{"kind": "unknown", "message": "Desktop relay disconnected; inspect the Mac before retrying."})
		}
		if !validReply(reply) {
			return errors.New("invalid window helper response")
		}
		if err := out.Encode(reply); err != nil {
			return err
		}
	}
	return lines.Err()
}

func validReply(wire json.RawMessage) bool {
	var reply struct{ Kind string }
	if json.Unmarshal(wire, &reply) != nil {
		return false
	}
	switch reply.Kind {
	case "observed", "frame", "denied", "reselect", "unknown":
		return true
	default:
		return false
	}
}

type httpFailure int

func (e httpFailure) Error() string { return fmt.Sprintf("request returned HTTP %d", int(e)) }

func requestJSON(ctx context.Context, client *http.Client, method, target, token string, body []byte, into any) error {
	req, err := http.NewRequestWithContext(ctx, method, target, bytes.NewReader(body))
	if err != nil {
		return err
	}
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	res, err := client.Do(req)
	if err != nil {
		return err
	}
	defer res.Body.Close()
	if res.StatusCode < 200 || res.StatusCode >= 300 {
		return httpFailure(res.StatusCode)
	}
	return json.NewDecoder(io.LimitReader(res.Body, 1600000)).Decode(into)
}

const keychainService = "app.jeeves.cloud-device"

func deviceKey(ctx context.Context, origin *url.URL) string {
	wire, err := exec.CommandContext(ctx, "/usr/bin/security", "find-generic-password", "-s", keychainService, "-a", origin.Host, "-w").Output()
	if err != nil {
		return ""
	}
	token := strings.TrimSpace(string(wire))
	if len(token) == 64 {
		return token
	}
	return ""
}
func saveDeviceKey(ctx context.Context, origin *url.URL, token string) error {
	cmd := exec.CommandContext(ctx, "/usr/bin/security", "-i")
	cmd.Stdin = strings.NewReader(fmt.Sprintf("add-generic-password -U -s %s -a %s -w %s\n", keychainService, origin.Host, token))
	cmd.Stdout, cmd.Stderr = io.Discard, io.Discard
	if cmd.Run() != nil || deviceKey(ctx, origin) != token {
		return errors.New("could not save the device credential in Keychain")
	}
	return nil
}

func runMac(ctx context.Context, origin *url.URL, helper string) error {
	client := &http.Client{Timeout: 6 * time.Second, CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}
	token := deviceKey(ctx, origin)
	var status struct{ Paired bool }
	if token != "" {
		err := requestJSON(ctx, client, "GET", origin.String()+"/api/mac/status", token, nil, &status)
		if err == httpFailure(http.StatusForbidden) || err == httpFailure(http.StatusUnauthorized) || err == nil && !status.Paired {
			token = ""
		}
	}
	if token == "" {
		host, err := os.Hostname()
		if err != nil || host == "" {
			host = "Mac"
		}
		wire, err := json.Marshal(map[string]string{"name": host})
		if err != nil {
			return err
		}
		var pairing struct{ Credential, URL string }
		if err := requestJSON(ctx, client, "POST", origin.String()+"/api/mac/register", "", wire, &pairing); err != nil {
			return err
		}
		link, err := url.Parse(pairing.URL)
		if err != nil || link.Host != origin.Host || link.Scheme != origin.Scheme || link.Path != "/device/" || len(pairing.Credential) != 64 {
			return errors.New("invalid device invitation")
		}
		if err := exec.CommandContext(ctx, "/usr/bin/open", link.String()).Run(); err != nil {
			return errors.New("could not open the account browser")
		}
		fmt.Println("pairing")
		token = pairing.Credential
		timed, end := context.WithTimeout(ctx, 10*time.Minute)
		defer end()
		for !status.Paired {
			select {
			case <-timed.Done():
				return errors.New("device approval expired")
			case <-time.After(2 * time.Second):
			}
			err := requestJSON(timed, client, "GET", origin.String()+"/api/mac/status", token, nil, &status)
			if err == httpFailure(http.StatusForbidden) || err == httpFailure(http.StatusUnauthorized) {
				return err
			}
		}
		if err := saveDeviceKey(ctx, origin, token); err != nil {
			return err
		}
	}
	for ctx.Err() == nil {
		err := runMacConnection(ctx, origin, token, helper)
		if errors.Is(err, rejected) {
			return errors.New("the Mac has been disconnected in Jeeves")
		}
		if ctx.Err() != nil {
			return ctx.Err()
		}
		fmt.Println("offline")
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(3 * time.Second):
		}
	}
	return ctx.Err()
}

type windowProcess struct {
	cmd  *exec.Cmd
	in   io.WriteCloser
	out  *bufio.Scanner
	link string
}

func (p *windowProcess) stop() {
	if p.cmd != nil {
		p.in.Close()
		p.cmd.Process.Kill()
		p.cmd.Wait()
		*p = windowProcess{}
	}
}
func (p *windowProcess) exchange(ctx context.Context, helper string, request deviceRequest) json.RawMessage {
	failed := json.RawMessage(`{"kind":"unknown","message":"Window helper disconnected. Check the last action before retrying."}`)
	cmd, ok := desktopInput(request.Command)
	if !ok {
		return json.RawMessage(`{"kind":"denied","message":"Unsupported device action."}`)
	}
	if cmd.Kind == "select" {
		p.stop()
		job := exec.CommandContext(ctx, helper)
		job.Env = []string{"HOME=" + os.Getenv("HOME"), "TMPDIR=" + os.Getenv("TMPDIR"), "LANG=" + os.Getenv("LANG")}
		in, err := job.StdinPipe()
		if err != nil {
			return failed
		}
		out, err := job.StdoutPipe()
		if err != nil {
			in.Close()
			return failed
		}
		if err := job.Start(); err != nil {
			in.Close()
			out.Close()
			return failed
		}
		p.cmd, p.in, p.out, p.link = job, in, bufio.NewScanner(out), request.Link
		p.out.Buffer(make([]byte, 4096), 1500000)
	}
	if p.link != request.Link || p.cmd == nil {
		return json.RawMessage(`{"kind":"reselect","message":"Choose a Mac window for this task."}`)
	}
	if _, err := p.in.Write(append(request.Command, '\n')); err != nil || !p.out.Scan() {
		p.stop()
		return failed
	}
	wire := bytes.Clone(p.out.Bytes())
	if !validReply(wire) {
		p.stop()
		return failed
	}
	return wire
}
func runMacConnection(ctx context.Context, origin *url.URL, token, helper string) error {
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	link := *origin
	link.Scheme = "wss"
	link.Path = "/api/mac/socket"
	conn, reply, err := websocket.Dial(ctx, link.String(), &websocket.DialOptions{HTTPHeader: http.Header{"Authorization": {"Bearer " + token}}})
	if err != nil {
		if reply != nil && (reply.StatusCode == 403 || reply.StatusCode == 401) {
			return rejected
		}
		return err
	}
	defer conn.CloseNow()
	conn.SetReadLimit(16 << 10)
	fmt.Println("online")
	requests := make(chan deviceRequest)
	go func() {
		defer cancel()
		for {
			var next deviceRequest
			if wsjson.Read(ctx, conn, &next) != nil {
				return
			}
			select {
			case requests <- next:
			case <-ctx.Done():
				return
			}
		}
	}()
	var process windowProcess
	defer process.stop()
	for {
		select {
		case <-ctx.Done():
			return ctx.Err()
		case request := <-requests:
			fmt.Println("executing")
			result := process.exchange(ctx, helper, request)
			if err := wsjson.Write(ctx, conn, deviceReply{ID: request.ID, Reply: result}); err != nil {
				return err
			}
			fmt.Println("online")
		}
	}
}
