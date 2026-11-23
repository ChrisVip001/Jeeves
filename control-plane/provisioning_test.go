package main

import (
	"encoding/json"
	"strings"
	"testing"
)

func TestDefaultProvisioningKeepsTheSingleIncludedProfile(t *testing.T) {
	t.Setenv("CP_MODEL_PROFILES", "")
	p, err := loadProvisioning("https://model.internal")
	if err != nil {
		t.Fatal(err)
	}
	if p.Active != "included" || len(p.Profiles) != 1 {
		t.Fatalf("unexpected default: %+v", p)
	}
	seed := p.Profiles[0]
	if seed.ID != "included" || seed.Origin != "platform" || seed.Line != "language" {
		t.Fatalf("unexpected seeded profile: %+v", seed)
	}
	if seed.BaseURL != "https://model.internal" || seed.KeySource != "env:JEEVES_MODEL_TOKEN" {
		t.Fatalf("the seed must point at the platform proxy: %+v", seed)
	}
}

func TestProvisioningSeedsOneProfilePerLineAndTier(t *testing.T) {
	raw := `{
		"active": "platform-language-chat",
		"profiles": [
			{"id":"platform-language-chat","provider":"anthropic","model":"claude","origin":"platform","line":"language"},
			{"id":"platform-vision","provider":"anthropic","model":"claude-vision","origin":"platform","line":"multimodal"},
			{"id":"platform-video-gen","provider":"anthropic","model":"video","origin":"platform","line":"video-generation"}
		]
	}`
	t.Setenv("CP_MODEL_PROFILES", raw)
	p, err := loadProvisioning("https://model.internal")
	if err != nil {
		t.Fatal(err)
	}
	if len(p.Profiles) != 3 || p.Active != "platform-language-chat" {
		t.Fatalf("unexpected profiles: %+v", p)
	}
	for _, profile := range p.Profiles {
		if profile.BaseURL != "https://model.internal" {
			t.Fatalf("deployment config must not override the proxy URL: %+v", profile)
		}
		if profile.Origin != "platform" || profile.KeySource != "env:JEEVES_MODEL_TOKEN" {
			t.Fatalf("seeded profiles are platform-owned: %+v", profile)
		}
	}

	data, err := modelsSeedJSON(p)
	if err != nil {
		t.Fatal(err)
	}
	var round map[string]any
	if err := json.Unmarshal(data, &round); err != nil {
		t.Fatalf("seeded models.json must be valid JSON: %s", data)
	}
	if round["active"] != "platform-language-chat" {
		t.Fatalf("wrong active profile in seed: %s", data)
	}
	profiles, ok := round["profiles"].([]any)
	if !ok || len(profiles) != 3 {
		t.Fatalf("wrong profiles in seed: %s", data)
	}
	first := profiles[0].(map[string]any)
	if first["origin"] != "platform" || first["line"] != "language" {
		t.Fatalf("origin/line must reach models.json: %s", data)
	}
}

func TestProvisioningRejectsInvalidConfiguration(t *testing.T) {
	modelURL := "https://model.internal"
	cases := map[string]string{
		"broken json":      `{`,
		"empty profiles":   `{"active":"x","profiles":[]}`,
		"no active":        `{"profiles":[{"id":"x","provider":"anthropic"}]}`,
		"unknown active":   `{"active":"y","profiles":[{"id":"x","provider":"anthropic"}]}`,
		"duplicate id":     `{"active":"x","profiles":[{"id":"x","provider":"anthropic"},{"id":"x","provider":"openai"}]}`,
		"missing id":       `{"active":"x","profiles":[{"provider":"anthropic"}]}`,
		"missing provider": `{"active":"x","profiles":[{"id":"x"}]}`,
		"byok origin":      `{"active":"x","profiles":[{"id":"x","provider":"anthropic","origin":"byok"}]}`,
	}
	for name, raw := range cases {
		t.Run(name, func(t *testing.T) {
			t.Setenv("CP_MODEL_PROFILES", raw)
			if _, err := loadProvisioning(modelURL); err == nil {
				t.Fatal("invalid provisioning must fail at startup, not at tenant creation")
			}
		})
	}
}

func TestSeedDoesNotLeakKeys(t *testing.T) {
	t.Setenv("CP_MODEL_PROFILES", "")
	p, err := loadProvisioning("https://model.internal")
	if err != nil {
		t.Fatal(err)
	}
	data, err := modelsSeedJSON(p)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(data), "sk-") {
		t.Fatalf("the seed must carry a key pointer only: %s", data)
	}
}
