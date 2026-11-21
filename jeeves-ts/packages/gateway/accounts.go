package main

import (
	"context"
	"crypto/hmac"
	"crypto/sha1"
	"database/sql"
	"encoding/base32"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"time"

	"golang.org/x/crypto/scrypt"
	"jeeves/internal/cloudkit"
)

var rejected = errors.New("invalid credentials or expired request")
var schema = []string{
	`CREATE TABLE IF NOT EXISTS gw_users (
		id TEXT PRIMARY KEY, email TEXT NOT NULL UNIQUE, salt TEXT NOT NULL, hash TEXT NOT NULL,
		verified BOOLEAN NOT NULL, created BIGINT NOT NULL, revision BIGINT NOT NULL DEFAULT 0,
		factor TEXT NOT NULL DEFAULT '', factor_last BIGINT NOT NULL DEFAULT -1, backups TEXT NOT NULL DEFAULT '[]')`,
	`CREATE TABLE IF NOT EXISTS gw_sessions (
		digest TEXT PRIMARY KEY, id TEXT NOT NULL UNIQUE, owner TEXT NOT NULL REFERENCES gw_users(id) ON DELETE CASCADE,
		revision BIGINT NOT NULL, issued BIGINT NOT NULL, expires BIGINT NOT NULL, client TEXT NOT NULL)`,
	`CREATE INDEX IF NOT EXISTS gw_sessions_owner ON gw_sessions(owner)`,
	`CREATE TABLE IF NOT EXISTS gw_challenges (
		id TEXT PRIMARY KEY, purpose TEXT NOT NULL, identity TEXT NOT NULL,
		digest TEXT NOT NULL, expires BIGINT NOT NULL)`,
	`CREATE INDEX IF NOT EXISTS gw_challenges_identity ON gw_challenges(identity, purpose)`,
	`CREATE INDEX IF NOT EXISTS gw_challenges_expiry ON gw_challenges(expires)`,
	`CREATE TABLE IF NOT EXISTS gw_rates (id TEXT PRIMARY KEY, window BIGINT NOT NULL, attempts BIGINT NOT NULL)`,
	`CREATE TABLE IF NOT EXISTS gw_oidc (
		issuer TEXT NOT NULL, subject TEXT NOT NULL, owner TEXT NOT NULL REFERENCES gw_users(id) ON DELETE CASCADE,
		PRIMARY KEY (issuer, subject))`,
	`CREATE TABLE IF NOT EXISTS gw_pending_factors (
		session_id TEXT PRIMARY KEY REFERENCES gw_sessions(id) ON DELETE CASCADE, secret TEXT NOT NULL, expires BIGINT NOT NULL)`,
}

type store struct {
	db  *sql.DB
	key []byte
	box cloudkit.Box
	kdf chan struct{}
}
type user struct {
	id, email, salt, hash, factor, backups string
	created, revision, last                int64
	verified                               bool
}
type session struct {
	ID       string `json:"id"`
	Client   string `json:"client"`
	Issued   int64  `json:"issued"`
	Expires  int64  `json:"expires"`
	owner    string
	revision int64
}

func newStore(ctx context.Context, db *sql.DB, key, encryption []byte) (*store, error) {
	if len(key) < 32 || len(encryption) < 32 || hmac.Equal(key, encryption) {
		return nil, errors.New("configure independent session and storage keys of at least 32 bytes")
	}
	box, err := cloudkit.Encryption(cloudkit.MAC(encryption, "jeeves-gateway"))
	if err != nil {
		return nil, err
	}
	if err := cloudkit.Schema(ctx, db, schema); err != nil {
		return nil, err
	}
	return &store{db: db, key: key, box: box, kdf: make(chan struct{}, 4)}, nil
}

const userColumns = "id,email,salt,hash,verified,created,revision,factor,factor_last,backups"

func scanUser(row *sql.Row) (user, error) {
	var a user
	err := row.Scan(&a.id, &a.email, &a.salt, &a.hash, &a.verified, &a.created, &a.revision, &a.factor, &a.last, &a.backups)
	return a, err
}
func (s *store) email(ctx context.Context, email string) (user, error) {
	return scanUser(s.db.QueryRowContext(ctx, "SELECT "+userColumns+" FROM gw_users WHERE email=$1", email))
}
func (s *store) digest(token string) string {
	return hex.EncodeToString(cloudkit.MAC(s.key, token))
}
func (s *store) password(ctx context.Context, password, salt string) (string, error) {
	select {
	case s.kdf <- struct{}{}:
		defer func() { <-s.kdf }()
	case <-ctx.Done():
		return "", ctx.Err()
	}
	hash, err := scrypt.Key([]byte(password), []byte(salt), 1<<14, 8, 1, 32)
	return hex.EncodeToString(hash), err
}
func (s *store) checkPassword(ctx context.Context, a user, password string) error {
	if len(password) > 256 || password == "" {
		return rejected
	}
	salt := a.salt
	if salt == "" {
		salt = "00"
	}
	check, err := s.password(ctx, password, salt)
	if err != nil {
		return err
	}
	if a.hash == "" || !hmac.Equal([]byte(check), []byte(a.hash)) {
		return rejected
	}
	return nil
}

func (s *store) create(ctx context.Context, tx *sql.Tx, email, salt, hash string, verified bool) (user, error) {
	a := user{id: "u-" + cloudkit.Token(6), email: email, salt: salt, hash: hash, verified: verified, created: time.Now().UnixMilli(), last: -1, backups: "[]"}
	res, err := tx.ExecContext(ctx, `INSERT INTO gw_users(id,email,salt,hash,verified,created) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(email) DO NOTHING`,
		a.id, email, salt, hash, verified, a.created)
	if err != nil {
		return user{}, err
	}
	return a, one(res)
}

func (s *store) session(ctx context.Context, a user, client string) (string, error) {
	token := cloudkit.Token(32)
	now := time.Now()
	_, err := s.db.ExecContext(ctx, `INSERT INTO gw_sessions(digest,id,owner,revision,issued,expires,client) VALUES($1,$2,$3,$4,$5,$6,$7)`,
		s.digest(token), cloudkit.Token(16), a.id, a.revision, now.UnixMilli(), now.Add(14*24*time.Hour).UnixMilli(), client)
	return token, err
}

func (s *store) authenticate(ctx context.Context, token string) (session, user, error) {
	var a user
	var ss session
	if len(token) != 64 {
		return ss, a, rejected
	}
	err := s.db.QueryRowContext(ctx, `SELECT s.id,s.client,s.issued,s.expires,s.owner,s.revision,
		u.id,u.email,u.salt,u.hash,u.verified,u.created,u.revision,u.factor,u.factor_last,u.backups
		FROM gw_sessions s JOIN gw_users u ON u.id=s.owner AND u.revision=s.revision
		WHERE s.digest=$1 AND s.expires>$2 AND u.verified`, s.digest(token), time.Now().UnixMilli()).Scan(
		&ss.ID, &ss.Client, &ss.Issued, &ss.Expires, &ss.owner, &ss.revision,
		&a.id, &a.email, &a.salt, &a.hash, &a.verified, &a.created, &a.revision, &a.factor, &a.last, &a.backups)
	return ss, a, err
}

func (s *store) list(ctx context.Context, owner string) ([]session, error) {
	rows, err := s.db.QueryContext(ctx, `SELECT s.id,s.client,s.issued,s.expires FROM gw_sessions s
	JOIN gw_users u ON u.id=s.owner AND u.revision=s.revision WHERE s.owner=$1 AND s.expires>$2 ORDER BY s.issued DESC`, owner, time.Now().UnixMilli())
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	list := []session{}
	for rows.Next() {
		var ss session
		if err := rows.Scan(&ss.ID, &ss.Client, &ss.Issued, &ss.Expires); err != nil {
			return nil, err
		}
		list = append(list, ss)
	}
	return list, rows.Err()
}

func (s *store) rate(ctx context.Context, id string, max int) (bool, error) {
	window := time.Now().Unix() / 60
	var attempts int
	err := s.db.QueryRowContext(ctx, `INSERT INTO gw_rates(id,window,attempts) VALUES($1,$2,1)
	 ON CONFLICT(id) DO UPDATE SET attempts=CASE WHEN gw_rates.window=excluded.window THEN gw_rates.attempts+1 ELSE 1 END, window=excluded.window RETURNING attempts`, s.digest(id), window).Scan(&attempts)
	return attempts <= max, err
}

func one(result sql.Result) error {
	n, err := result.RowsAffected()
	if err != nil {
		return err
	}
	if n != 1 {
		return rejected
	}
	return nil
}

func (s *store) challenge(ctx context.Context, purpose, who, code string, ttl time.Duration) (string, error) {
	id := cloudkit.Token(16)
	_, err := s.db.ExecContext(ctx, `INSERT INTO gw_challenges(id,purpose,identity,digest,expires) VALUES($1,$2,$3,$4,$5)`,
		id, purpose, who, s.digest(code), time.Now().Add(ttl).UnixMilli())
	return id, err
}

func (s *store) consume(ctx context.Context, tx *sql.Tx, token, purpose, who string) error {
	id, code, ok := strings.Cut(token, ".")
	if !ok || len(id) != 32 || len(code) < 6 || len(code) > 128 {
		return rejected
	}
	result, err := tx.ExecContext(ctx, `DELETE FROM gw_challenges WHERE id=$1 AND purpose=$2 AND identity=$3 AND digest=$4 AND expires>$5`,
		id, purpose, who, s.digest(code), time.Now().UnixMilli())
	if err != nil {
		return err
	}
	return one(result)
}

func (s *store) setPassword(ctx context.Context, tx *sql.Tx, a user, salt, hash string) error {
	result, err := tx.ExecContext(ctx, `UPDATE gw_users SET salt=$1, hash=$2, revision=revision+1 WHERE id=$3 AND revision=$4`, salt, hash, a.id, a.revision)
	if err != nil {
		return err
	}
	if err := one(result); err != nil {
		return err
	}
	_, err = tx.ExecContext(ctx, "DELETE FROM gw_sessions WHERE owner=$1", a.id)
	return err
}

func totp(secret string, step int64) string {
	key, err := base32.StdEncoding.WithPadding(base32.NoPadding).DecodeString(secret)
	if err != nil {
		return ""
	}
	counter := make([]byte, 8)
	binary.BigEndian.PutUint64(counter, uint64(step))
	h := hmac.New(sha1.New, key)
	h.Write(counter)
	sum := h.Sum(nil)
	offset := sum[len(sum)-1] & 15
	return fmt.Sprintf("%06d", (binary.BigEndian.Uint32(sum[offset:offset+4])&0x7fffffff)%1000000)
}
func acceptStep(secret, code string, last int64) (int64, bool) {
	now := time.Now().Unix() / 30
	if len(code) != 6 {
		return 0, false
	}
	for _, step := range []int64{now, now - 1, now + 1} {
		if step > last && hmac.Equal([]byte(totp(secret, step)), []byte(code)) {
			return step, true
		}
	}
	return 0, false
}
func (s *store) factorUse(ctx context.Context, tx *sql.Tx, a user, code string) error {
	if a.factor == "" {
		return nil
	}
	secret, err := s.box.Open(a.factor, a.id)
	if err != nil {
		return err
	}
	step, ok := acceptStep(secret, code, a.last)
	used := a.backups
	if !ok {
		var hashes []string
		if err := json.Unmarshal([]byte(a.backups), &hashes); err != nil {
			return err
		}
		found := false
		for i, digest := range hashes {
			if hmac.Equal([]byte(digest), []byte(cloudkit.Digest(code))) {
				hashes = append(hashes[:i], hashes[i+1:]...)
				found = true
				break
			}
		}
		if !found {
			return rejected
		}
		wire, err := json.Marshal(hashes)
		if err != nil {
			return err
		}
		used = string(wire)
		step = a.last
	}
	res, err := tx.ExecContext(ctx, `UPDATE gw_users SET factor_last=$1, backups=$2 WHERE id=$3 AND revision=$4 AND factor_last=$5 AND backups=$6`,
		step, used, a.id, a.revision, a.last, a.backups)
	if err != nil {
		return err
	}
	return one(res)
}
