package main

import (
	"bytes"
	"context"
	"crypto/tls"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"net"
	"net/http"
	"net/mail"
	"net/smtp"
	"net/url"
	"os"
	"strings"
	"time"
)

type mailJob struct{ email, purpose, token, code string }

func (g *gateway) mailWorker(ctx context.Context) {
	for {
		select {
		case <-ctx.Done():
			return
		case job := <-g.mailJobs:
			timed, end := context.WithTimeout(ctx, 12*time.Second)
			if err := g.mail(timed, job); err != nil {
				log.Printf("email delivery failed (%T)", err)
			}
			end()
		}
	}
}

func configureMail(origin *url.URL) (func(context.Context, mailJob) error, error) {
	from, err := mail.ParseAddress(os.Getenv("GW_MAIL_FROM"))
	if err != nil {
		return nil, errors.New("configure GW_MAIL_FROM")
	}
	var send func(context.Context, string, string, string) error
	if raw := os.Getenv("GW_SMTP_URL"); raw != "" {
		u, err := url.Parse(raw)
		if err != nil || u.User != nil || u.Hostname() == "" || (u.Scheme != "smtp" && u.Scheme != "smtps") {
			return nil, errors.New("configure a credential-free smtp or smtps endpoint")
		}
		user, password := os.Getenv("GW_SMTP_USER"), os.Getenv("GW_SMTP_PASSWORD")
		if user == "" || password == "" {
			return nil, errors.New("configure SMTP credentials")
		}
		send = func(ctx context.Context, to, subject, text string) error {
			return sendSMTP(ctx, u, user, password, from.Address, to, subject, text)
		}
	} else {
		client := &http.Client{CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}
		key := os.Getenv("GW_RESEND_API_KEY")
		endpoint := "https://api.resend.com/emails"
		if os.Getenv("GW_DEVELOPMENT") == "1" && os.Getenv("GW_TEST_MAIL_URL") != "" {
			endpoint = os.Getenv("GW_TEST_MAIL_URL")
		}
		send = func(ctx context.Context, to, subject, text string) error {
			wire, err := json.Marshal(map[string]any{"from": from.String(), "to": []string{to}, "subject": subject, "text": text})
			if err != nil {
				return err
			}
			req, err := http.NewRequestWithContext(ctx, "POST", endpoint, bytes.NewReader(wire))
			if err != nil {
				return err
			}
			req.Header.Set("Authorization", "Bearer "+key)
			req.Header.Set("Content-Type", "application/json")
			reply, err := client.Do(req)
			if err != nil {
				return err
			}
			defer reply.Body.Close()
			if reply.StatusCode < 200 || reply.StatusCode >= 300 {
				return fmt.Errorf("mail provider returned HTTP %d", reply.StatusCode)
			}
			return nil
		}
	}
	return func(ctx context.Context, j mailJob) error {
		link := origin.ResolveReference(&url.URL{Path: "/account/"})
		fragment := url.Values{"action": {j.purpose}, "email": {j.email}, "token": {j.token}}.Encode()
		subject := "Verify your Jeeves email"
		if j.purpose == "reset" {
			subject = "Reset your Jeeves password"
		}
		return send(ctx, j.email, subject, subject+"\n\n"+link.String()+"#"+fragment+"\n\n"+j.code+"\n\nValid for 15 minutes. Ignore if you did not request this.\n链接和验证码 15 分钟内有效。")
	}, nil
}

func sendSMTP(ctx context.Context, endpoint *url.URL, user, password, from, to, subject, body string) error {
	port := endpoint.Port()
	if port == "" {
		port = "587"
		if endpoint.Scheme == "smtps" {
			port = "465"
		}
	}
	address := net.JoinHostPort(endpoint.Hostname(), port)
	cfg := &tls.Config{ServerName: endpoint.Hostname(), MinVersion: tls.VersionTLS12}
	dialer := &net.Dialer{}
	var conn net.Conn
	var err error
	if endpoint.Scheme == "smtps" {
		conn, err = (&tls.Dialer{NetDialer: dialer, Config: cfg}).DialContext(ctx, "tcp", address)
	} else {
		conn, err = dialer.DialContext(ctx, "tcp", address)
	}
	if err != nil {
		return err
	}
	defer conn.Close()
	if deadline, ok := ctx.Deadline(); ok {
		if err := conn.SetDeadline(deadline); err != nil {
			return err
		}
	}
	client, err := smtp.NewClient(conn, endpoint.Hostname())
	if err != nil {
		return err
	}
	defer client.Close()
	if endpoint.Scheme == "smtp" {
		if err := client.StartTLS(cfg); err != nil {
			return err
		}
	}
	var auth smtp.Auth = smtp.PlainAuth("", user, password, endpoint.Hostname())
	if strings.EqualFold(os.Getenv("GW_SMTP_AUTH"), "LOGIN") {
		auth = loginAuth{user, password}
	}
	if err := client.Auth(auth); err != nil {
		return err
	}
	if err := client.Mail(from); err != nil {
		return err
	}
	if err := client.Rcpt(to); err != nil {
		return err
	}
	out, err := client.Data()
	if err != nil {
		return err
	}
	data := "From: " + from + "\r\nTo: " + to + "\r\nSubject: " + subject + "\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=UTF-8\r\n\r\n" + strings.ReplaceAll(body, "\n", "\r\n")
	if _, err := out.Write([]byte(data)); err != nil {
		return err
	}
	if err := out.Close(); err != nil {
		return err
	}
	return client.Quit()
}

type loginAuth struct{ user, password string }

func (a loginAuth) Start(s *smtp.ServerInfo) (string, []byte, error) {
	if !s.TLS {
		return "", nil, errors.New("SMTP TLS is required")
	}
	return "LOGIN", nil, nil
}
func (a loginAuth) Next(prompt []byte, more bool) ([]byte, error) {
	if !more {
		return nil, nil
	}
	switch strings.ToLower(strings.TrimSpace(string(prompt))) {
	case "username:":
		return []byte(a.user), nil
	case "password:":
		return []byte(a.password), nil
	default:
		return nil, errors.New("unexpected SMTP login prompt")
	}
}
