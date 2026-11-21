package main

import (
	"context"
	"crypto/aes"
	"crypto/cipher"
	"crypto/hkdf"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"jeeves/internal/cloudkit"
)

func TestLegacyLoopbackImportPreservesAndDoesNotReviveSessions(t *testing.T) {
	ctx := context.Background()
	g := testGateway(t)
	g.fixed = cloudkit.RuntimeEndpoint{URL: "http://127.0.0.1:7777"}
	g.loopbackOnly = true
	g.fixedOwner = "owner@example.test"
	now := time.Now()
	token := base64.RawURLEncoding.EncodeToString(make([]byte, 32))
	backup := []legacySession{{ID: strings.Repeat("a", 32), AccountID: "u-000000000001", Digest: g.store.digest(token),
		Client: "Mac", Issued: now.Add(-time.Minute).UnixMilli(), Expires: now.Add(time.Hour).UnixMilli()}}
	clear, err := json.Marshal(map[string]any{
		"accounts": []legacyAccount{{ID: backup[0].AccountID, Email: g.fixedOwner,
			Salt: "legacy-salt", Hash: strings.Repeat("0", 64), CreatedAt: json.RawMessage(`"2026-11-21T09:57:22.123Z"`), Verified: true}},
		"sessions": backup,
	})
	if err != nil {
		t.Fatal(err)
	}
	const oldKey = "previous-gateway-storage-key-for-test"
	key, err := hkdf.Key(sha256.New, []byte(oldKey), nil, "jeeves/account-state/v1", 32)
	if err != nil {
		t.Fatal(err)
	}
	block, err := aes.NewCipher(key)
	if err != nil {
		t.Fatal(err)
	}
	box, err := cipher.NewGCM(block)
	if err != nil {
		t.Fatal(err)
	}
	iv := make([]byte, box.NonceSize())
	sealed := box.Seal(nil, iv, clear, nil)
	data, tag := sealed[:len(sealed)-box.Overhead()], sealed[len(sealed)-box.Overhead():]
	wrap, err := json.Marshal(map[string]any{"format": "jeeves-accounts-v1", "iv": iv, "data": data, "tag": tag})
	if err != nil {
		t.Fatal(err)
	}
	file := filepath.Join(t.TempDir(), "accounts.json")
	if err := os.WriteFile(file, wrap, 0600); err != nil {
		t.Fatal(err)
	}
	t.Setenv("GW_LEGACY_STATE_KEY", oldKey)
	t.Setenv("GW_IMPORT_SESSIONS", "1")
	if err := g.importLegacy(ctx, file); err != nil {
		t.Fatal(err)
	}
	ss, a, err := g.store.authenticate(ctx, token)
	if err != nil || ss.ID != backup[0].ID || a.id != backup[0].AccountID || a.created != 1795255042123 {
		t.Fatalf("the legacy account or its session changed: %v", err)
	}
	if _, err := g.store.db.ExecContext(ctx, "DELETE FROM gw_sessions WHERE id=$1", ss.ID); err != nil {
		t.Fatal(err)
	}
	if err := g.importLegacy(ctx, file); err != nil {
		t.Fatal(err)
	}
	if _, _, err := g.store.authenticate(ctx, token); err == nil {
		t.Fatal("reimport revived a revoked browser session")
	}
}
