package main

import (
	"bufio"
	"bytes"
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"io"
	"log"
	"net/http"
	"strings"
	"time"

	"jeeves/internal/cloudkit"
)

var limitReached = errors.New("额度用完，请充值或配置自己的模型")

func period() string { return time.Now().UTC().Format("2006-01") }

func (p *plane) usage(w http.ResponseWriter, r *http.Request) {
	if !cloudkit.Authorized(r, p.gatewayKey) {
		cloudkit.Error(w, 401, "gateway authentication required")
		return
	}
	var req cloudkit.RuntimeRequest
	if cloudkit.Decode(w, r, 2048, &req) != nil || !cloudkit.TenantPattern.MatchString(req.Tenant) {
		cloudkit.Error(w, 400, "invalid tenant")
		return
	}
	var result cloudkit.ModelUsage
	if p.fixed.URL == "" {
		result.Quota = &p.quota
		err := p.db.QueryRowContext(r.Context(), "SELECT used FROM cp_usage WHERE tenant=$1 AND period=$2", req.Tenant, period()).Scan(&result.Used)
		if err != nil && !errors.Is(err, sql.ErrNoRows) {
			internalError(w, err)
			return
		}
	}
	cloudkit.JSON(w, 200, result)
}
func (p *plane) reserve(ctx context.Context, id, tenant, month string, amount int64) error {
	tx, err := p.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	if _, err := tx.ExecContext(ctx, `INSERT INTO cp_usage(tenant,period) VALUES($1,$2) ON CONFLICT DO NOTHING`, tenant, month); err != nil {
		return err
	}
	result, err := tx.ExecContext(ctx, `UPDATE cp_usage SET reserved=reserved+$1 WHERE tenant=$2 AND period=$3 AND used+reserved+$1<=$4`, amount, tenant, month, p.quota)
	if err != nil {
		return err
	}
	count, err := result.RowsAffected()
	if err != nil {
		return err
	}
	if count != 1 {
		return limitReached
	}
	_, err = tx.ExecContext(ctx, `INSERT INTO cp_reservations(id,tenant,period,amount,created,state) VALUES($1,$2,$3,$4,$5,'pending')`,
		id, tenant, month, amount, time.Now().UnixMilli())
	if err != nil {
		return err
	}
	return tx.Commit()
}
func (p *plane) finalize(ctx context.Context, id string, actual int64) error {
	tx, err := p.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	var who, month string
	var amount int64
	err = tx.QueryRowContext(ctx, `UPDATE cp_reservations SET state='settled',actual=$1 WHERE id=$2 AND state IN ('pending','unknown') RETURNING tenant,period,amount`, actual, id).Scan(&who, &month, &amount)
	if errors.Is(err, sql.ErrNoRows) {
		return nil
	}
	if err != nil {
		return err
	}
	_, err = tx.ExecContext(ctx, `UPDATE cp_usage SET reserved=reserved-$1,used=used+$2 WHERE tenant=$3 AND period=$4`, amount, actual, who, month)
	if err != nil {
		return err
	}
	return tx.Commit()
}

func (p *plane) model(w http.ResponseWriter, r *http.Request) {
	if p.provider == nil {
		cloudkit.Error(w, 404, "model service unavailable")
		return
	}
	token := cloudkit.Bearer(r)
	var who string
	err := p.db.QueryRowContext(r.Context(), "SELECT id FROM cp_tenants WHERE model_digest=$1", cloudkit.Digest(token)).Scan(&who)
	if errors.Is(err, sql.ErrNoRows) || token == "" {
		cloudkit.Error(w, 401, "model token required")
		return
	}
	if err != nil {
		internalError(w, err)
		return
	}
	body, err := io.ReadAll(http.MaxBytesReader(w, r.Body, 2<<20))
	if err != nil {
		cloudkit.Error(w, 413, "model request too large")
		return
	}
	type block struct {
		Type string `json:"type"`
	}
	type message struct {
		Content json.RawMessage `json:"content"`
	}
	var input struct {
		MaxTokens int64     `json:"max_tokens"`
		Messages  []message `json:"messages"`
	}
	if json.Unmarshal(body, &input) != nil || input.MaxTokens <= 0 || input.MaxTokens > 131072 {
		cloudkit.Error(w, 400, "a bounded max_tokens is required")
		return
	}
	for _, m := range input.Messages {
		var blocks []block
		if json.Unmarshal(m.Content, &blocks) != nil {
			var content string
			if json.Unmarshal(m.Content, &content) != nil {
				cloudkit.Error(w, 400, "invalid model message content")
				return
			}
		}
		for _, item := range blocks {
			switch item.Type {
			case "image", "document", "audio":
				cloudkit.Error(w, 400, "the included model expects text messages")
				return
			}
		}
	}
	id := cloudkit.Token(16)
	allowance := int64(len(body))*2 + input.MaxTokens + 256
	if err := p.reserve(r.Context(), id, who, period(), allowance); err != nil {
		if errors.Is(err, limitReached) {
			cloudkit.Error(w, 429, "额度用完，请充值或配置自己的模型")
			return
		}
		internalError(w, err)
		return
	}
	actual := int64(-1)
	defer func() {
		final, cancel := context.WithTimeout(context.Background(), 4*time.Second)
		defer cancel()
		var err error
		if actual < 0 {
			_, err = p.db.ExecContext(final, "UPDATE cp_reservations SET state='unknown' WHERE id=$1 AND state='pending'", id)
		} else {
			err = p.finalize(final, id, actual)
		}
		if err != nil {
			log.Printf("model usage reservation %s needs reconciliation (%T)", id, err)
		}
	}()
	link := *p.provider
	link.Path = "/v1/messages"
	ctx, cancel := context.WithTimeout(r.Context(), 5*time.Minute)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, "POST", link.String(), bytes.NewReader(body))
	if err != nil {
		actual = 0
		internalError(w, err)
		return
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("x-api-key", p.providerKey)
	for _, header := range []string{"Anthropic-Version", "Anthropic-Beta", "Accept"} {
		if value := r.Header.Get(header); value != "" {
			req.Header.Set(header, value)
		}
	}
	result, err := p.modelHTTP.Do(req)
	if err != nil {
		cloudkit.Error(w, 502, "model provider unavailable")
		return
	}
	defer result.Body.Close()
	content := result.Header.Get("Content-Type")
	w.Header().Set("Content-Type", content)
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(result.StatusCode)
	if result.StatusCode >= 400 && result.StatusCode < 500 {
		actual = 0
		io.Copy(w, io.LimitReader(result.Body, 1<<20))
		return
	}
	tally := usage{}
	if strings.HasPrefix(content, "text/event-stream") {
		reader := bufio.NewReaderSize(result.Body, 64<<10)
		for {
			line, readErr := reader.ReadSlice('\n')
			if len(line) > 0 {
				if _, err := w.Write(line); err != nil {
					return
				}
				if err := http.NewResponseController(w).Flush(); err != nil {
					return
				}
				if data, ok := bytes.CutPrefix(line, []byte("data:")); ok {
					tally.feed(data)
				}
			}
			if errors.Is(readErr, io.EOF) {
				if tally.seen && tally.completed {
					actual = tally.input + tally.output
				}
				return
			}
			if readErr != nil {
				return
			}
		}
	}
	bytes, err := io.ReadAll(io.LimitReader(result.Body, 8<<20))
	if err != nil {
		return
	}
	if _, err := w.Write(bytes); err != nil {
		return
	}
	if tally.feed(bytes) && tally.seen {
		actual = tally.input + tally.output
	}
}

type usage struct {
	input, output   int64
	seen, completed bool
}
type usagePair struct {
	Input  *int64 `json:"input_tokens"`
	Output *int64 `json:"output_tokens"`
}

func (u *usage) feed(data []byte) bool {
	var frame struct {
		Type    string
		Usage   usagePair
		Message struct{ Usage usagePair }
	}
	if json.Unmarshal(bytes.TrimSpace(data), &frame) != nil {
		return false
	}
	for _, p := range []usagePair{frame.Usage, frame.Message.Usage} {
		if p.Input != nil && *p.Input >= 0 && *p.Input <= 1<<40 {
			u.input = max(u.input, *p.Input)
			u.seen = true
		}
		if p.Output != nil && *p.Output >= 0 && *p.Output <= 1<<40 {
			u.output = max(u.output, *p.Output)
			u.seen = true
		}
	}
	u.completed = u.completed || frame.Type == "message_stop"
	return true
}
