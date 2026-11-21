package cloudkit

import (
	"context"
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"database/sql"
	"encoding/base64"
	"fmt"
	"strings"
	"time"

	_ "github.com/jackc/pgx/v5/stdlib"
	"github.com/mattn/go-sqlite3"
)

func Open(ctx context.Context, dsn string) (*sql.DB, error) {
	driver := "pgx"
	if strings.HasPrefix(dsn, "sqlite:") {
		driver = "sqlite3"
		dsn = strings.TrimPrefix(dsn, "sqlite:") + "?_foreign_keys=on&_busy_timeout=5000&_journal_mode=WAL"
	}
	db, err := sql.Open(driver, dsn)
	if err != nil {
		return nil, err
	}
	db.SetMaxOpenConns(16)
	db.SetConnMaxIdleTime(2 * time.Minute)
	if driver == "sqlite3" {
		db.SetMaxOpenConns(1)
	}
	if err := db.PingContext(ctx); err != nil {
		db.Close()
		return nil, fmt.Errorf("database unavailable: %w", err)
	}
	return db, nil
}

func Schema(ctx context.Context, db *sql.DB, statements []string) error {
	tx, err := db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	if _, sqlite := db.Driver().(*sqlite3.SQLiteDriver); !sqlite {
		if _, err := tx.ExecContext(ctx, "SELECT pg_advisory_xact_lock(13820921)"); err != nil {
			return err
		}
	}
	for _, ddl := range statements {
		if _, err := tx.ExecContext(ctx, ddl); err != nil {
			return err
		}
	}
	return tx.Commit()
}

type Box struct{ aead cipher.AEAD }

func Encryption(key []byte) (Box, error) {
	block, err := aes.NewCipher(key)
	if err != nil {
		return Box{}, err
	}
	aead, err := cipher.NewGCM(block)
	return Box{aead}, err
}

func (b Box) Seal(value, purpose string) string {
	nonce := make([]byte, b.aead.NonceSize())
	rand.Read(nonce)
	return base64.RawURLEncoding.EncodeToString(
		b.aead.Seal(nonce, nonce, []byte(value), []byte(purpose)),
	)
}

func (b Box) Open(value, purpose string) (string, error) {
	wire, err := base64.RawURLEncoding.DecodeString(value)
	if err != nil || len(wire) < b.aead.NonceSize()+b.aead.Overhead() {
		return "", fmt.Errorf("invalid encrypted value")
	}
	size := b.aead.NonceSize()
	clear, err := b.aead.Open(nil, wire[:size], wire[size:], []byte(purpose))
	return string(clear), err
}
