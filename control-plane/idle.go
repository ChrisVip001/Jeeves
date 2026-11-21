package main

import (
	"context"
	"encoding/json"
	"io"
	"log"
	"net/http"
	"net/url"
)

func report(id string, err error) { log.Printf("reconcile Runtime %s: %v", id, err) }
func (p *plane) probeJSON(ctx context.Context, t tenant, path string, into any) bool {
	req, err := http.NewRequestWithContext(ctx, "GET", p.cluster.url(t.id)+path, nil)
	if err != nil {
		return false
	}
	req.Header.Set("Authorization", "Bearer "+t.ops)
	res, err := p.probe.Do(req)
	if err != nil {
		return false
	}
	defer res.Body.Close()
	return res.StatusCode == 200 && json.NewDecoder(io.LimitReader(res.Body, 4<<20)).Decode(into) == nil
}
func (p *plane) settled(ctx context.Context, t tenant) bool {
	var active struct{ Turns []json.RawMessage }
	if !p.probeJSON(ctx, t, "/api/turns", &active) || active.Turns == nil || len(active.Turns) != 0 {
		return false
	}
	var scheduled struct {
		Jobs  []json.RawMessage
		Error *string
	}
	if !p.probeJSON(ctx, t, "/api/schedules/jobs", &scheduled) || scheduled.Jobs == nil || scheduled.Error != nil || len(scheduled.Jobs) != 0 {
		return false
	}
	cursor := ""
	for range 100 {
		var page struct {
			Tasks      []struct{ Status string }
			NextCursor string
		}
		if !p.probeJSON(ctx, t, "/api/tasks?cursor="+url.QueryEscape(cursor), &page) || page.Tasks == nil {
			return false
		}
		for _, task := range page.Tasks {
			switch task.Status {
			case "done", "failed", "cancelled":
			default:
				return false
			}
		}
		if page.NextCursor == "" {
			return true
		}
		if page.NextCursor == cursor {
			return false
		}
		cursor = page.NextCursor
	}
	return false
}
