package main

import (
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"time"

	"jeeves/internal/cloudkit"
)

func (g *gateway) discover(ctx context.Context) (cloudkit.RuntimeEndpoint, error) {
	var target cloudkit.RuntimeEndpoint
	file, err := os.Open(filepath.Join(g.runtimeHome, "runtime-endpoint.json"))
	if errors.Is(err, os.ErrNotExist) {
		return target, nil
	}
	if err != nil {
		return target, fmt.Errorf("open the local Runtime endpoint: %w", err)
	}
	defer file.Close()
	var record struct {
		Port     uint16 `json:"port"`
		Instance string `json:"instance"`
	}
	if err := json.NewDecoder(io.LimitReader(file, 512)).Decode(&record); err != nil {
		return target, fmt.Errorf("read the local Runtime endpoint: %w", err)
	}
	if record.Port == 0 || len(record.Instance) != 32 {
		return target, errors.New("invalid local Runtime endpoint")
	}

	// A crashed Runtime's port can be reused. Confirm its identity before sending the bearer.
	url := fmt.Sprintf("http://127.0.0.1:%d", record.Port)
	ctx, cancel := context.WithTimeout(ctx, time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodHead, url+"/internal/runtime-ready", nil)
	if err != nil {
		return target, err
	}
	resp, err := g.http.Do(req)
	if err != nil {
		return target, nil
	}
	defer resp.Body.Close()
	proof, err := hex.DecodeString(resp.Header.Get("X-Jeeves-Runtime-Proof"))
	mac := hmac.New(sha256.New, []byte(g.runtimeToken))
	mac.Write([]byte(record.Instance))
	if resp.StatusCode != http.StatusNoContent || err != nil || !hmac.Equal(mac.Sum(nil), proof) {
		return target, nil
	}
	return cloudkit.RuntimeEndpoint{URL: url, Token: g.runtimeToken, Ready: true}, nil
}
