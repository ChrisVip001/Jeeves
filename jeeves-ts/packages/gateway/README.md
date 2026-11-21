# Jeeves unified gateway

A cloud load balancer or ingress terminates public HTTPS. The Go gateway serves the Web and iOS workspaces, owns accounts and sessions, and routes authenticated requests to the user's Rust Runtime. The private Go control plane returns the Runtime endpoint for the authenticated tenant. The Mac device app pairs through the same account and grants access to a selected window. [Architecture](../../../docs/architecture.md)

## Cloud configuration

Build from the repository root with `docker build -f jeeves-ts/packages/gateway/Dockerfile -t jeeves-gateway .`. The image serves the built dashboard and listens on port 8080. See the [hosted deployment guide](../../../control-plane/README.md) for the other images and infrastructure.

| Environment | Purpose |
| --- | --- |
| `GW_PUBLIC_ORIGIN` | The HTTPS origin installed clients and email users open |
| `GW_DATABASE_URL` | PostgreSQL for accounts, sessions, verification, and device pairing |
| `GW_SESSION_KEY`, `GW_STATE_KEY` | Independent random secrets of at least 32 bytes, shared by gateway replicas |
| `GW_CONTROL_URL`, `GW_CONTROL_TOKEN` | Private Go control-plane origin and service credential |
| `GW_MAIL_FROM` and either `GW_SMTP_URL` or `GW_RESEND_API_KEY` | Verified sender and transport for email verification and recovery |
| `GW_OIDC_ISSUER`, `GW_OIDC_CLIENT_ID`, `GW_OIDC_CLIENT_SECRET` | Optional OpenID Connect provider. Register the exact HTTPS callback `/api/oidc/callback` on the public origin |
| `GW_TRUST_PROXY` | Set to `1` behind a private trusted ingress that preserves Host and overwrites forwarding headers |
| `GW_ADDR`, `GW_TLS_CERT`, `GW_TLS_KEY` | Listener address and certificates for direct HTTPS termination when used |

SMTP accepts `smtp://` with required STARTTLS and `smtps://` for TLS on connection. Provide credentials with `GW_SMTP_USER` and `GW_SMTP_PASSWORD`. An OIDC identity is keyed by issuer and subject. The gateway refuses an automatic match to an existing password account with the same email. Password login supports authenticator codes and single-use recovery codes.

## Routing and credentials

The gateway uses a host-only, HTTPS-only, HTTP-only cookie for browser sessions. It derives the Runtime destination from the authenticated account and supplies the Runtime credential server-side. Keep the control plane, tenant Services and database private. Preserve streaming responses, WebSocket upgrades and long enough idle timeouts at the public ingress. Use TLS or a service mesh for traffic crossing internal trust boundaries.

Channel adapters run within an authorized tenant deployment. Their configuration and scoped credentials are documented in [the Runtime shell guide](../../../docs/internals/07-shells.md#78-runtime-hosted-lark-gateway).

## Personal installation

Run `just local-up` at the repository root with Go, Rust and Node installed. It opens a one-time owner setup in the browser and binds both processes to loopback. Return to the printed address after setup. The launcher stores Runtime data and the account database in `~/.local/share/jeeves-personal` by default. Personal mode uses a locally configured model and is accessible only on the host computer. Its account is separate from a hosted account.

To replace an existing loopback HTTPS account gateway for a fixed owner, keep its public origin and trusted certificate and point `GW_RUNTIME_URL` and `GW_RUNTIME_TOKEN` at the current Runtime. Set `GW_FIXED_OWNER_EMAIL` and use a private SQLite file for `GW_DATABASE_URL`. This configuration listens only on loopback and keeps secure browser cookies. During the [legacy account import](../../../control-plane/README.md#account-and-task-migration), supply the previous session key as `GW_SESSION_KEY` and set `GW_IMPORT_SESSIONS=1` to preserve unexpired sessions for a newly imported owner. Keep the original encrypted account file as a rollback copy and remove the import options before starting the listener.

## Verification

Run `just go-check` for the Go packages. Run `just smoke-account` on a machine with Chrome for browser registration, recovery and task continuity across independent desktop and phone-sized sessions. The smoke uses its own data, a simulated model and a test mail inbox. To exercise PostgreSQL, set `JEEVES_TEST_GATEWAY_DATABASE` and `JEEVES_TEST_CONTROL_DATABASE` to fresh local `jeeves_test_gateway` and `jeeves_test_control` databases. The control-plane Go tests also accept the latter variable.
