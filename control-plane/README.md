# Jeeves control plane

The cloud shell around the unchanged Jeeves engine: **one Pod and one volume per user** (silo
multi-tenancy). No dependencies; Node ≥ 22.18 runs the TypeScript directly.

| Piece | File | Does |
|---|---|---|
| Accounts | `src/accounts.ts` | email + scrypt password, signed session cookie, per-user ops and model tokens, monthly usage ledger |
| Manifests | `src/manifests.ts` | Secret + PVC + Deployment (`Recreate`, replicas 1 awake / 0 asleep) + Service per user |
| Cluster | `src/cluster.ts` | `kubectl apply/scale/delete`; an in-memory twin for tests |
| Model proxy | `src/proxy.ts` | `/v1/*`: checks the per-user token and monthly quota, adds the platform key, counts tokens |
| Server | `src/server.ts` | register/login, `/app/*` routed to the user's own Pod, wake on demand, idle sleep |

Rules that hold the design together:

- **The platform model key never enters a user Pod.** The Pod gets its own ops token and a
  per-user model token; `models.json` points at this proxy and names the token by `env:` pointer.
  A test fails if a key-shaped field appears in the rendered manifests.
- **Sleep keeps the volume.** Accounts with no active app writes and only terminal tasks across all task-list pages can scale to zero. A failed task check keeps the Pod awake. Idle accounts scale to zero (`CP_IDLE_MINUTES`, default 20) after
  Jeeves's own 25 s drain; the next request wakes the Pod. `alwaysOn` accounts are skipped, which is
  how schedules and chat channels stay up 24 h.
- **Over quota, say so.** The proxy answers 429 with "add your own model key in Settings → Models";
  a user who adds their own key bypasses the platform quota entirely.
- **Moving house**: `jeeves home export <dir>` locally, `jeeves home import <dir>` in the Pod (and the
  reverse). Keys and sign-ins are given again; they are never exported.

Configuration (environment): `CP_SESSION_SECRET`, `CP_IMAGE`, `CP_MODEL_URL`, `CP_MODEL_UPSTREAM`,
`CP_MODEL_KEY`, optional `CP_STATE`, `CP_NAMESPACE`, `CP_IDLE_MINUTES`, `CP_MONTHLY_TOKENS`, `CP_PORT`.
Deploy with `deploy/k8s/control-plane.yaml`. Test: `npm test` in this directory.

Not here yet: payment, email verification, a UI (the endpoints are the contract), TLS (terminate at
your ingress), per-user network policy (add one denying Pod-to-Pod traffic in your cluster).
