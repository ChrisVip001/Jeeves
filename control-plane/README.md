# Jeeves cloud deployment

The public HTTPS entry sends traffic to the [Go unified gateway](../gateway/README.md). Deploy the private Go control plane beside it. Kubernetes runs and isolates each tenant's Rust Runtime, and the control plane meters the included model. [Architecture](../docs/architecture.md)

## Build and install

Build all three images from the repository root.

```bash
docker build -f gateway/Dockerfile -t jeeves-gateway .
docker build -f control-plane/Dockerfile -t jeeves-control-plane .
docker build -t jeeves .
```

The gateway image contains the Web workbench. The Runtime image contains the channel adapters and their Node Runtime client.

Use the [Kubernetes example](../deploy/k8s/control-plane.yaml) with a private PostgreSQL instance, a default StorageClass, a NetworkPolicy-capable CNI and an HTTPS ingress. Substitute the domain, image references and ingress class for the installation. Provision `gateway-env`, `cp-env` and the public TLS certificate in the Jeeves namespace. Use separate databases or roles for the gateway and control plane. Enable database TLS and encrypted backups in production. The example's ingress rules expose the gateway Service. Enforce private network access and apply [the mTLS example](../deploy/k8s/runtime-mtls.yaml) on an Istio installation. Restrict tenant egress according to the approved tool and connector policy.

| Control-plane environment | Purpose |
| --- | --- |
| `CP_DATABASE_URL`, `CP_STATE_KEY` | PostgreSQL and an independent credential encryption secret of at least 32 bytes |
| `CP_GATEWAY_TOKEN` | Service credential matching the gateway's `GW_CONTROL_TOKEN` |
| `CP_IMAGE`, `CP_NAMESPACE` | Rust Runtime image and Kubernetes namespace |
| `CP_MODEL_URL`, `CP_MODEL_UPSTREAM`, `CP_MODEL_KEY` | Private model proxy origin and the HTTPS provider origin and platform key for the Anthropic Messages API |
| `CP_MONTHLY_TOKENS`, `CP_IDLE` | Monthly token allowance and idle interval, defaults to 2 million and 20 minutes |
| `CP_DESKTOP_GATEWAY_URL` | Private gateway origin for an authorized Runtime's Mac relay |

## Tenant lifecycle and quotas

The first workspace visit requests a tenant Runtime. The control plane reconciles one Deployment, one Service, scoped credentials and a persistent volume claim for that tenant. The gateway waits for readiness. Every tenant Pod has Kubernetes CPU and memory bounds, a Runtime turn limit, and a workspace storage ceiling. Once the Runtime is idle, the control plane verifies active turns, tasks and schedules before scaling down. The persistent volume survives sleep.

The proxy reserves a conservative allowance before forwarding a model request. It reconciles reservations once to the provider's reported usage and retains uncertain outcomes for operator review. The included quota is inferred from the configured policy and belongs to each tenant. Limits on SaaS plan purchases, invoices, and provider charges require a separate billing integration.

## Data and backups

PostgreSQL stores accounts, tenants and usage. Each tenant's persistent volume holds tasks and memory. Back up both stores and protect their credentials. After restoring, sign in with the same account and confirm the existing task IDs and deliveries. See [Runtime data and backups](../docs/operations.md#data-and-backups).
