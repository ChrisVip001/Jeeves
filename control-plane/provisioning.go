package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"strings"
)

// modelProfileSeed is one line of a tenant's seeded models.json. These fields mirror
// jeeves-config's ModelProfile; origin/line are always platform-owned here.
type modelProfileSeed struct {
	ID         string `json:"id"`
	Provider   string `json:"provider"`
	Model      string `json:"model,omitempty"`
	SmallModel string `json:"small_model,omitempty"`
	BaseURL    string `json:"base_url,omitempty"`
	KeySource  string `json:"key_source,omitempty"`
	Origin     string `json:"origin"`
	Line       string `json:"line,omitempty"`
}

// provisioning is the platform's answer to "which models does a new tenant get".
// It is configuration (CP_MODEL_PROFILES), not a build flag: a deployment decides the
// lines and tiers it serves without changing code.
type provisioning struct {
	Active   string             `json:"active"`
	Profiles []modelProfileSeed `json:"profiles"`
}

// loadProvisioning reads CP_MODEL_PROFILES. When unset, a single `included` profile is
// seeded, preserving the behaviour of older control-plane deployments.
func loadProvisioning(modelURL string) (provisioning, error) {
	raw := strings.TrimSpace(os.Getenv("CP_MODEL_PROFILES"))
	if raw == "" {
		return defaultProvisioning(modelURL), nil
	}
	var p provisioning
	if err := json.Unmarshal([]byte(raw), &p); err != nil {
		return provisioning{}, fmt.Errorf("CP_MODEL_PROFILES is not valid JSON: %w", err)
	}
	if len(p.Profiles) == 0 {
		return provisioning{}, errors.New("CP_MODEL_PROFILES has no profiles")
	}
	ids := make(map[string]bool, len(p.Profiles))
	for _, profile := range p.Profiles {
		id := strings.TrimSpace(profile.ID)
		if id == "" {
			return provisioning{}, errors.New("CP_MODEL_PROFILES has a profile with no id")
		}
		if ids[id] {
			return provisioning{}, fmt.Errorf("CP_MODEL_PROFILES repeats id '%s'", id)
		}
		ids[id] = true
		if strings.TrimSpace(profile.Provider) == "" {
			return provisioning{}, fmt.Errorf("profile '%s' has no provider", id)
		}
		if profile.Origin != "" && profile.Origin != "platform" {
			return provisioning{}, fmt.Errorf("profile '%s' declares origin '%s'; seeded profiles are always platform", id, profile.Origin)
		}
	}
	if strings.TrimSpace(p.Active) == "" {
		return provisioning{}, errors.New("CP_MODEL_PROFILES has no active profile")
	}
	if !ids[p.Active] {
		return provisioning{}, fmt.Errorf("active profile '%s' is not in the list", p.Active)
	}
	// A seeded profile always points at the platform proxy and takes its credential from
	// the injected token. Deployment config may pick id/provider/model/line, not the endpoint.
	for i := range p.Profiles {
		p.Profiles[i].BaseURL = modelURL
		p.Profiles[i].KeySource = "env:JEEVES_MODEL_TOKEN"
		p.Profiles[i].Origin = "platform"
		if p.Profiles[i].Line == "" {
			p.Profiles[i].Line = "language"
		}
	}
	return p, nil
}

func defaultProvisioning(modelURL string) provisioning {
	return provisioning{
		Active: "included",
		Profiles: []modelProfileSeed{{
			ID: "included", Provider: "anthropic", BaseURL: modelURL,
			KeySource: "env:JEEVES_MODEL_TOKEN", Origin: "platform", Line: "language",
		}},
	}
}

// modelsSeedJSON builds the models.json an initContainer writes into a new tenant home.
func modelsSeedJSON(p provisioning) ([]byte, error) {
	return json.Marshal(p)
}
