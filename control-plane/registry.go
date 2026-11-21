package main

import (
	"context"
	"database/sql"
	"errors"
	"time"

	"jeeves/internal/cloudkit"
)

var schema = []string{
	`CREATE TABLE IF NOT EXISTS cp_tenants (
		id TEXT PRIMARY KEY, ops TEXT NOT NULL, model TEXT NOT NULL, model_digest TEXT NOT NULL UNIQUE,
		wanted BOOLEAN NOT NULL, ready BOOLEAN NOT NULL, last_active BIGINT NOT NULL,
		lease_until BIGINT NOT NULL DEFAULT 0, next_check BIGINT NOT NULL DEFAULT 0, always_on BOOLEAN NOT NULL DEFAULT FALSE)`,
	`CREATE TABLE IF NOT EXISTS cp_usage (
		tenant TEXT NOT NULL REFERENCES cp_tenants(id) ON DELETE CASCADE, period TEXT NOT NULL,
		used BIGINT NOT NULL DEFAULT 0, reserved BIGINT NOT NULL DEFAULT 0, PRIMARY KEY(tenant,period))`,
	`CREATE TABLE IF NOT EXISTS cp_reservations (
		id TEXT PRIMARY KEY, tenant TEXT NOT NULL, period TEXT NOT NULL, amount BIGINT NOT NULL,
		created BIGINT NOT NULL, state TEXT NOT NULL, actual BIGINT,
		FOREIGN KEY(tenant,period) REFERENCES cp_usage(tenant,period))`,
}

type tenant struct {
	id, ops, model          string
	wanted, ready, alwaysOn bool
	last, lease             int64
}
type registry struct {
	db  *sql.DB
	box cloudkit.Box
}

func (s registry) open(ctx context.Context, id string) (tenant, error) {
	var t tenant
	err := s.db.QueryRowContext(ctx, `SELECT id,ops,model,wanted,ready,last_active,lease_until,always_on FROM cp_tenants WHERE id=$1`, id).
		Scan(&t.id, &t.ops, &t.model, &t.wanted, &t.ready, &t.last, &t.lease, &t.alwaysOn)
	if err != nil {
		return t, err
	}
	t.ops, err = s.box.Open(t.ops, id+":ops")
	if err != nil {
		return t, err
	}
	t.model, err = s.box.Open(t.model, id+":model")
	return t, err
}
func (s registry) save(ctx context.Context, id, ops, model string, want bool) error {
	_, err := s.db.ExecContext(ctx, `INSERT INTO cp_tenants(id,ops,model,model_digest,wanted,ready,last_active,next_check) VALUES($1,$2,$3,$4,$5,FALSE,$6,CASE WHEN $5 THEN 0 ELSE -1 END)
	ON CONFLICT(id) DO NOTHING`, id, s.box.Seal(ops, id+":ops"), s.box.Seal(model, id+":model"), cloudkit.Digest(model), want, time.Now().UnixMilli())
	return err
}
func (s registry) wake(ctx context.Context, id string) (tenant, error) {
	t, err := s.open(ctx, id)
	if errors.Is(err, sql.ErrNoRows) {
		if err := s.save(ctx, id, cloudkit.Token(24), cloudkit.Token(32), true); err != nil {
			return t, err
		}
		return s.open(ctx, id)
	}
	if err != nil {
		return t, err
	}
	now := time.Now().UnixMilli()
	if !t.wanted || now-t.last >= time.Minute.Milliseconds()/4 {
		_, err = s.db.ExecContext(ctx, `UPDATE cp_tenants SET last_active=$1,next_check=CASE WHEN wanted THEN next_check ELSE 0 END,wanted=TRUE
		WHERE id=$2`, now, id)
		t.wanted = true
	}
	return t, err
}
func (s registry) due(ctx context.Context) ([]string, error) {
	now := time.Now().UnixMilli()
	rows, err := s.db.QueryContext(ctx, `SELECT id FROM cp_tenants WHERE next_check<=$1 AND lease_until<$1 AND (wanted OR ready OR next_check=0)
		ORDER BY next_check LIMIT 64`, now)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var ids []string
	for rows.Next() {
		var id string
		if err := rows.Scan(&id); err != nil {
			return nil, err
		}
		ids = append(ids, id)
	}
	return ids, rows.Err()
}
func (s registry) claim(ctx context.Context, id string) (tenant, error) {
	now := time.Now().UnixMilli()
	res, err := s.db.ExecContext(ctx, `UPDATE cp_tenants SET lease_until=$1 WHERE id=$2 AND lease_until<$3 AND next_check<=$3`, now+30_000, id, now)
	if err != nil {
		return tenant{}, err
	}
	n, err := res.RowsAffected()
	if err != nil || n != 1 {
		return tenant{}, err
	}
	return s.open(ctx, id)
}

func (s registry) result(ctx context.Context, t tenant, ready bool, delay time.Duration) error {
	next := time.Now().Add(delay).UnixMilli()
	_, err := s.db.ExecContext(ctx, `UPDATE cp_tenants SET ready=$1,lease_until=0,next_check=CASE WHEN wanted=$2 THEN CAST($3 AS BIGINT) ELSE 0 END
		WHERE id=$4 AND lease_until=$5`, ready, t.wanted, next, t.id, t.lease)
	return err
}
