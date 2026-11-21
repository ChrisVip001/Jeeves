package cloudkit

type RuntimeRequest struct {
	Tenant string `json:"tenant"`
}
type RuntimeEndpoint struct {
	URL   string `json:"url"`
	Token string `json:"token"`
	Ready bool   `json:"ready"`
}

type ModelUsage struct {
	Used  int64  `json:"used"`
	Quota *int64 `json:"quota"`
}

type WorkspaceState struct {
	Ready   bool `json:"ready"`
	Managed bool `json:"managed"`
}
