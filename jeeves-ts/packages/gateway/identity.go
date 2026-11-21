package main

import (
	"context"
	"crypto/rand"
	"database/sql"
	"encoding/base32"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"math/big"
	"net"
	"net/http"
	"regexp"
	"strings"
	"time"

	"jeeves/internal/cloudkit"
)

var emailPattern = regexp.MustCompile(`^[^@\s]+@[^@\s]+\.[^@\s]+$`)
var sessionID = regexp.MustCompile(`^[a-f0-9]{32}$`)

type accountFields struct {
	Email, Password, Token, Code, Current, Replacement string
}

func (g *gateway) cookie(w http.ResponseWriter, token string, age int) {
	http.SetCookie(w, &http.Cookie{
		Name: g.cookieName(), Value: token, Path: "/", MaxAge: age, Secure: !g.local, HttpOnly: true, SameSite: http.SameSiteLaxMode,
	})
}
func (g *gateway) cookieName() string {
	if g.local {
		return "jeeves_local_session"
	}
	return "__Host-jeeves_session"
}
func (g *gateway) sessionToken(r *http.Request) string {
	cookie, err := r.Cookie(g.cookieName())
	if err != nil {
		return ""
	}
	return cookie.Value
}
func (g *gateway) rate(w http.ResponseWriter, r *http.Request, id string, max int) bool {
	ok, err := g.store.rate(r.Context(), id, max)
	if err != nil {
		failure(w, err)
		return false
	}
	if !ok {
		w.Header().Set("Retry-After", "60")
		cloudkit.Error(w, 429, "try again in a minute")
	}
	return ok
}
func failure(w http.ResponseWriter, err error) {
	if errors.Is(err, rejected) || errors.Is(err, sql.ErrNoRows) {
		cloudkit.Error(w, 403, "invalid or expired request")
		return
	}
	log.Printf("gateway request failed (%T)", err)
	cloudkit.Error(w, 503, "service temporarily unavailable")
}
func clientName(r *http.Request) string {
	ua := r.UserAgent()
	if strings.Contains(ua, "iPhone") || strings.Contains(ua, "iPad") {
		return "iOS"
	}
	if strings.Contains(ua, "Macintosh") || strings.Contains(ua, "Mac OS X") {
		return "Mac"
	}
	return "Web"
}
func (g *gateway) issue(w http.ResponseWriter, r *http.Request, a user) {
	ctx := r.Context()
	if old := g.sessionToken(r); old != "" {
		if _, err := g.store.db.ExecContext(ctx, "DELETE FROM gw_sessions WHERE digest=$1", g.store.digest(old)); err != nil {
			failure(w, err)
			return
		}
	}
	token, err := g.store.session(ctx, a, clientName(r))
	if err != nil {
		failure(w, err)
		return
	}
	g.cookie(w, token, 14*86400)
	cloudkit.JSON(w, 200, map[string]string{"id": a.id})
}
func (g *gateway) clientIP(r *http.Request) string {
	if g.trustProxy {
		forwarded := strings.Join(r.Header.Values("X-Forwarded-For"), ",")
		if ip := net.ParseIP(strings.TrimSpace(forwarded[strings.LastIndex(forwarded, ",")+1:])); ip != nil {
			return ip.String()
		}
	}
	ip, _, _ := net.SplitHostPort(r.RemoteAddr)
	return ip
}

func normalizeEmail(value string) string {
	return strings.ToLower(strings.TrimSpace(value))
}

func (g *gateway) identity(w http.ResponseWriter, r *http.Request) {
	ctx := r.Context()
	if !g.rate(w, r, "auth:"+g.clientIP(r), 240) {
		return
	}
	var body accountFields
	if cloudkit.Decode(w, r, 16<<10, &body) != nil {
		cloudkit.Error(w, 400, "invalid request")
		return
	}
	body.Email = normalizeEmail(body.Email)
	body.Code = strings.TrimSpace(body.Code)
	if !emailPattern.MatchString(body.Email) || len(body.Email) > 254 {
		cloudkit.Error(w, 400, "invalid email address")
		return
	}
	a, err := g.store.email(ctx, body.Email)
	if err != nil && !errors.Is(err, sql.ErrNoRows) {
		failure(w, err)
		return
	}
	switch r.URL.Path {
	case "/api/register", "/api/forgot", "/api/send-verification":
		g.sendMail(w, r, body.Email, a)
	case "/api/login":
		if g.fixedOwner != "" && a.email != g.fixedOwner {
			failure(w, rejected)
			return
		}
		if !g.rate(w, r, "credentials:"+body.Email, 15) {
			return
		}
		if err := g.store.checkPassword(ctx, a, body.Password); err != nil {
			failure(w, err)
			return
		}
		if !a.verified {
			cloudkit.JSON(w, 200, map[string]string{"step": "verify"})
			return
		}
		if a.factor != "" && body.Code == "" {
			cloudkit.JSON(w, 200, map[string]string{"step": "factor"})
			return
		}
		if a.factor != "" {
			err := g.transaction(ctx, func(tx *sql.Tx) error { return g.store.factorUse(ctx, tx, a, body.Code) })
			if err != nil {
				failure(w, err)
				return
			}
		}
		g.issue(w, r, a)
	case "/api/verify", "/api/reset", "/api/setup":
		if !g.rate(w, r, "proof:"+body.Email, 15) {
			return
		}
		g.finishIdentity(w, r, body, a)
	}
}
func (g *gateway) transaction(ctx context.Context, run func(*sql.Tx) error) error {
	tx, err := g.store.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	if err := run(tx); err != nil {
		return err
	}
	return tx.Commit()
}
func (g *gateway) sendMail(w http.ResponseWriter, r *http.Request, email string, a user) {
	if g.local {
		cloudkit.Error(w, 403, "use local account setup")
		return
	}
	purpose := "verify"
	if r.URL.Path == "/api/forgot" {
		purpose = "reset"
	}
	if !g.rate(w, r, "mail:"+email+":"+purpose, 3) {
		return
	}
	if g.mail == nil {
		cloudkit.Error(w, 503, "email is unavailable")
		return
	}
	code, err := rand.Int(rand.Reader, big.NewInt(1_000_000))
	if err != nil {
		failure(w, err)
		return
	}
	proof := fmt.Sprintf("%06d", code)
	destination := cloudkit.Token(16)
	shouldSend := (g.fixedOwner == "" || email == g.fixedOwner) && (purpose == "reset" && a.verified ||
		purpose == "verify" && !a.verified && (a.id != "" || r.URL.Path == "/api/register"))
	if shouldSend {
		destination, err = g.store.challenge(r.Context(), purpose, email, proof, 15*time.Minute)
		if err != nil {
			failure(w, err)
			return
		}
		select {
		case g.mailJobs <- mailJob{email, purpose, destination + "." + proof, proof}:
		default:
			cloudkit.Error(w, 503, "email delivery is busy")
			return
		}
	}
	cloudkit.JSON(w, 202, map[string]string{"step": "mail", "challenge": destination})
}

func newPassword(password string) bool { return len(password) >= 10 && len(password) <= 256 }
func (g *gateway) finishIdentity(w http.ResponseWriter, r *http.Request, body accountFields, a user) {
	ctx := r.Context()
	setup := r.URL.Path == "/api/setup"
	if g.fixedOwner != "" && body.Email != g.fixedOwner {
		failure(w, rejected)
		return
	}
	reset := r.URL.Path == "/api/reset"
	if !newPassword(body.Password) {
		cloudkit.Error(w, 400, "choose a password between 10 and 256 bytes")
		return
	}
	if setup {
		if !g.local || g.setup == "" || g.store.digest(body.Token) != g.store.digest(g.setup) {
			failure(w, rejected)
			return
		}
	} else if g.local || reset != a.verified || (reset && a.id == "") {
		failure(w, rejected)
		return
	}
	if !reset && a.id != "" {
		if err := g.store.checkPassword(ctx, a, body.Password); err != nil {
			failure(w, err)
			return
		}
	}
	salt := cloudkit.Token(16)
	hash, err := g.store.password(ctx, body.Password, salt)
	if err != nil {
		failure(w, err)
		return
	}
	err = g.transaction(ctx, func(tx *sql.Tx) error {
		if setup {
			var count int
			if err := tx.QueryRowContext(ctx, "SELECT COUNT(*) FROM gw_users").Scan(&count); err != nil {
				return err
			}
			if count > 0 {
				return rejected
			}
		} else {
			purpose := "verify"
			if reset {
				purpose = "reset"
			}
			if err := g.store.consume(ctx, tx, body.Token, purpose, body.Email); err != nil {
				return err
			}
		}
		if reset {
			if err := g.store.setPassword(ctx, tx, a, salt, hash); err != nil {
				return err
			}
		} else if a.id == "" {
			a, err = g.store.create(ctx, tx, body.Email, salt, hash, true)
			if err != nil {
				return err
			}
		} else {
			res, err := tx.ExecContext(ctx, "UPDATE gw_users SET verified=TRUE,revision=revision+1 WHERE id=$1 AND revision=$2 AND NOT verified", a.id, a.revision)
			if err != nil {
				return err
			}
			if err := one(res); err != nil {
				return err
			}
			a.revision++
		}
		_, err := tx.ExecContext(ctx, "DELETE FROM gw_challenges WHERE identity=$1 AND purpose IN ('verify','reset')", body.Email)
		return err
	})
	if err != nil {
		failure(w, err)
		return
	}
	if reset {
		cloudkit.JSON(w, 200, map[string]bool{"ok": true})
		return
	}
	if a.factor != "" {
		cloudkit.JSON(w, 200, map[string]string{"step": "factor"})
		return
	}
	g.issue(w, r, a)
}

func (g *gateway) accountMutation(w http.ResponseWriter, r *http.Request, a user, ss session) {
	ctx := r.Context()
	path := r.URL.Path
	if path == "/api/logout" || r.Method == "DELETE" {
		id := ss.ID
		if r.Method == "DELETE" {
			id = strings.TrimPrefix(path, "/api/account/sessions/")
		}
		if !sessionID.MatchString(id) {
			cloudkit.Error(w, 404, "session not found")
			return
		}
		res, err := g.store.db.ExecContext(ctx, "DELETE FROM gw_sessions WHERE owner=$1 AND id=$2", a.id, id)
		if err != nil {
			failure(w, err)
			return
		}
		if one(res) != nil {
			cloudkit.Error(w, 404, "session not found")
			return
		}
		if id == ss.ID {
			g.cookie(w, "", -1)
		}
		cloudkit.JSON(w, 200, map[string]bool{"ok": true})
		return
	}
	if !g.rate(w, r, "security:"+a.id, 10) {
		return
	}
	var body accountFields
	if cloudkit.Decode(w, r, 16<<10, &body) != nil {
		cloudkit.Error(w, 400, "invalid request")
		return
	}
	password := body.Password
	if path == "/api/account/password" {
		password = body.Current
	}
	if err := g.store.checkPassword(ctx, a, password); err != nil {
		failure(w, err)
		return
	}
	if path == "/api/account/password" {
		if !newPassword(body.Replacement) {
			cloudkit.Error(w, 400, "choose a longer password")
			return
		}
		salt := cloudkit.Token(16)
		hash, err := g.store.password(ctx, body.Replacement, salt)
		if err == nil {
			err = g.transaction(ctx, func(tx *sql.Tx) error { return g.store.setPassword(ctx, tx, a, salt, hash) })
		}
		if err != nil {
			failure(w, err)
			return
		}
		g.cookie(w, "", -1)
		cloudkit.JSON(w, 200, map[string]bool{"ok": true})
		return
	}
	g.manageFactor(w, r, a, ss, body.Code)
}
func (g *gateway) manageFactor(w http.ResponseWriter, r *http.Request, a user, ss session, code string) {
	ctx := r.Context()
	action := strings.TrimPrefix(r.URL.Path, "/api/account/factor/")
	if action == "start" {
		if a.factor != "" {
			cloudkit.Error(w, 409, "factor already enabled")
			return
		}
		bytes := make([]byte, 20)
		rand.Read(bytes)
		secret := base32.StdEncoding.WithPadding(base32.NoPadding).EncodeToString(bytes)
		_, err := g.store.db.ExecContext(ctx, `INSERT INTO gw_pending_factors(session_id,secret,expires) VALUES($1,$2,$3)
		ON CONFLICT(session_id) DO UPDATE SET secret=excluded.secret,expires=excluded.expires`, ss.ID, g.store.box.Seal(secret, ss.ID), time.Now().Add(10*time.Minute).UnixMilli())
		if err != nil {
			failure(w, err)
			return
		}
		cloudkit.JSON(w, 200, map[string]string{"secret": secret})
		return
	}
	codes := []string{}
	err := g.transaction(ctx, func(tx *sql.Tx) error {
		factor, backups, last := "", "[]", int64(-1)
		switch action {
		case "confirm":
			if a.factor != "" {
				return rejected
			}
			var encrypted string
			if err := tx.QueryRowContext(ctx, "SELECT secret FROM gw_pending_factors WHERE session_id=$1 AND expires>$2", ss.ID, time.Now().UnixMilli()).Scan(&encrypted); err != nil {
				return err
			}
			secret, err := g.store.box.Open(encrypted, ss.ID)
			if err != nil {
				return err
			}
			step, ok := acceptStep(secret, strings.TrimSpace(code), -1)
			if !ok {
				return rejected
			}
			last = step
			factor = g.store.box.Seal(secret, a.id)
			var hashes []string
			for range 8 {
				value := cloudkit.Token(10)
				codes = append(codes, value)
				hashes = append(hashes, cloudkit.Digest(value))
			}
			wire, err := json.Marshal(hashes)
			if err != nil {
				return err
			}
			backups = string(wire)
			if _, err := tx.ExecContext(ctx, "DELETE FROM gw_pending_factors WHERE session_id=$1", ss.ID); err != nil {
				return err
			}
		case "disable":
			if a.factor == "" {
				return rejected
			}
			if err := g.store.factorUse(ctx, tx, a, strings.TrimSpace(code)); err != nil {
				return err
			}
		default:
			return rejected
		}
		result, err := tx.ExecContext(ctx, `UPDATE gw_users SET factor=$1,backups=$2,factor_last=$3,revision=revision+1 WHERE id=$4 AND revision=$5`,
			factor, backups, last, a.id, a.revision)
		if err != nil {
			return err
		}
		if err := one(result); err != nil {
			return err
		}
		result, err = tx.ExecContext(ctx, "UPDATE gw_sessions SET revision=$1 WHERE id=$2", a.revision+1, ss.ID)
		if err != nil {
			return err
		}
		if err := one(result); err != nil {
			return err
		}
		_, err = tx.ExecContext(ctx, "DELETE FROM gw_sessions WHERE owner=$1 AND id<>$2", a.id, ss.ID)
		return err
	})
	if err != nil {
		failure(w, err)
		return
	}
	if action == "confirm" {
		cloudkit.JSON(w, 200, map[string]any{"codes": codes})
		return
	}
	cloudkit.JSON(w, 200, map[string]bool{"ok": true})
}
