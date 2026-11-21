package main

import (
	"bytes"
	"context"
	"crypto/aes"
	"crypto/cipher"
	"crypto/sha256"
	"database/sql"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"os"
	"time"

	"crypto/hkdf"
	"jeeves/internal/cloudkit"
)

type legacyAccount struct {
	ID, Email, Salt, Hash, OpsToken, ModelToken string
	Verified                                    bool
	AlwaysOn                                    bool
	CreatedAt                                   json.RawMessage
	Factor                                      *struct {
		Secret  string
		Last    int64
		Backups []string
	}
}

type legacySession struct {
	ID, AccountID, Digest, Client string
	Issued, Expires               int64
}

func (a legacyAccount) createdMillis() (int64, error) {
	var stamp time.Time
	if json.Unmarshal(a.CreatedAt, &stamp) == nil && !stamp.IsZero() {
		return stamp.UnixMilli(), nil
	}
	var millis int64
	if json.Unmarshal(a.CreatedAt, &millis) == nil && millis > 0 {
		return millis, nil
	}
	return 0, errors.New("invalid legacy registration time")
}

func (g *gateway) importLegacy(ctx context.Context, path string) error {
	input, err := os.ReadFile(path)
	if err != nil {
		return err
	}
	var wrapper struct{ Format, IV, Tag, Data string }
	if json.Unmarshal(input, &wrapper) != nil || wrapper.Format != "jeeves-accounts-v1" {
		return errors.New("expected an encrypted legacy account export")
	}
	key, err := hkdf.Key(sha256.New, []byte(os.Getenv("GW_LEGACY_STATE_KEY")), nil, "jeeves/account-state/v1", 32)
	if err != nil {
		return err
	}
	block, err := aes.NewCipher(key)
	if err != nil {
		return err
	}
	box, err := cipher.NewGCM(block)
	if err != nil {
		return err
	}
	iv, err := base64.StdEncoding.DecodeString(wrapper.IV)
	if err != nil || len(iv) != box.NonceSize() {
		return errors.New("invalid legacy encryption nonce")
	}
	tag, e1 := base64.StdEncoding.DecodeString(wrapper.Tag)
	data, e2 := base64.StdEncoding.DecodeString(wrapper.Data)
	if e1 != nil || e2 != nil {
		return errors.New("invalid legacy encryption envelope")
	}
	clear, err := box.Open(nil, iv, append(data, tag...), nil)
	if err != nil {
		return errors.New("could not decrypt the legacy accounts")
	}
	var previous struct {
		Accounts []legacyAccount
		Usage    map[string]map[string]int64
		Sessions []legacySession
	}
	if err := json.Unmarshal(clear, &previous); err != nil {
		return err
	}
	if g.fixed.URL != "" && len(previous.Accounts) > 1 {
		return errors.New("personal mode accepts a single owner")
	}
	keepSessions := os.Getenv("GW_IMPORT_SESSIONS") == "1"
	if keepSessions && (!g.loopbackOnly || g.local || g.fixed.URL == "" || g.fixedOwner == "") {
		return errors.New("session import requires a fixed owner's loopback HTTPS gateway")
	}
	if !keepSessions {
		previous.Sessions = nil
	}
	for _, a := range previous.Accounts {
		if g.fixedOwner != "" && a.Email != g.fixedOwner {
			return errors.New("legacy owner does not match the fixed Runtime")
		}
		if !cloudkit.TenantPattern.MatchString(a.ID) || len(a.Hash) != 64 || !emailPattern.MatchString(a.Email) {
			return errors.New("invalid legacy account record")
		}
		if g.controlURL != "" {
			if err := g.importTenant(ctx, a, previous.Usage[a.ID]); err != nil {
				return err
			}
		}
		if err := g.importUser(ctx, a, previous.Sessions); err != nil {
			return err
		}
	}
	return nil
}
func (g *gateway) importUser(ctx context.Context, a legacyAccount, sessions []legacySession) error {
	created, err := a.createdMillis()
	if err != nil {
		return err
	}
	factor, backups, last := "", "[]", int64(-1)
	if a.Factor != nil {
		factor = g.store.box.Seal(a.Factor.Secret, a.ID)
		wire, err := json.Marshal(a.Factor.Backups)
		if err != nil {
			return err
		}
		backups, last = string(wire), a.Factor.Last
	}
	return g.transaction(ctx, func(tx *sql.Tx) error {
		if g.fixed.URL != "" {
			var count int
			if err := tx.QueryRowContext(ctx, "SELECT COUNT(*) FROM gw_users WHERE id<>$1", a.ID).Scan(&count); err != nil {
				return err
			}
			if count != 0 {
				return errors.New("this Runtime already has another owner")
			}
		}
		result, err := tx.ExecContext(ctx, `INSERT INTO gw_users(id,email,salt,hash,verified,created,factor,factor_last,backups)
		 VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT(id) DO NOTHING`, a.ID, a.Email, a.Salt, a.Hash, a.Verified, created, factor, last, backups)
		if err != nil {
			return err
		}
		added, err := result.RowsAffected()
		if err != nil || added != 1 || !a.Verified {
			return err
		}
		now := time.Now().UnixMilli()
		for _, ss := range sessions {
			digest, err := hex.DecodeString(ss.Digest)
			if ss.AccountID != a.ID || ss.Expires <= now || ss.Expires <= ss.Issued || ss.Issued <= 0 || ss.Issued > now ||
				!sessionID.MatchString(ss.ID) || err != nil || len(digest) != sha256.Size {
				continue
			}
			if _, err := tx.ExecContext(ctx, `INSERT INTO gw_sessions(digest,id,owner,revision,issued,expires,client)
			 VALUES($1,$2,$3,0,$4,$5,$6)`, ss.Digest, ss.ID, a.ID, ss.Issued, ss.Expires, ss.Client); err != nil {
				return err
			}
		}
		return err
	})
}
func (g *gateway) importTenant(ctx context.Context, a legacyAccount, usage map[string]int64) error {
	data, err := json.Marshal(map[string]any{"tenant": a.ID, "ops": a.OpsToken, "model": a.ModelToken, "usage": usage, "alwaysOn": a.AlwaysOn})
	if err != nil {
		return err
	}
	req, err := http.NewRequestWithContext(ctx, "POST", g.controlURL+"/internal/import", bytes.NewReader(data))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Authorization", "Bearer "+g.controlToken)
	res, err := g.http.Do(req)
	if err != nil {
		return err
	}
	defer res.Body.Close()
	if res.StatusCode != 200 {
		return fmt.Errorf("control plane refused legacy tenant (%d)", res.StatusCode)
	}
	return nil
}
