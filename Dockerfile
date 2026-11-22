# Jeeves's container image: a multi-stage build; the runtime image keeps only the binary, the
# sandbox and CA certificates.
#
#   docker build -t jeeves .
#   Deploy on the private Runtime network. The Go gateway owns the public listener.
#   docker run --rm -it -v jeeves-state:/home/jeeves/.jeeves -e ANTHROPIC_API_KEY=… jeeves app-server
#
# All state (state.db, SOUL.md, skills, outbox, traces) lives in JEEVES_HOME, so one volume
# carries it.

# ---- Build -----------------------------------------------------------------
# Builder and runtime share one Debian release (trixie), so the binary's glibc matches.
FROM rust:1.100-slim-trixie AS builder
WORKDIR /src
# Manifests first, with stub sources: the dependency layer is cached by Docker, so a code
# change does not rebuild every dependency.
COPY Cargo.toml Cargo.lock ./
COPY runtime/crates/jeeves-agent/Cargo.toml ./runtime/crates/jeeves-agent/
COPY runtime/crates/jeeves-app-server/Cargo.toml ./runtime/crates/jeeves-app-server/
COPY runtime/crates/jeeves-cli/Cargo.toml ./runtime/crates/jeeves-cli/
COPY runtime/crates/jeeves-config/Cargo.toml ./runtime/crates/jeeves-config/
COPY runtime/crates/jeeves-connector/Cargo.toml ./runtime/crates/jeeves-connector/
COPY runtime/crates/jeeves-eval/Cargo.toml ./runtime/crates/jeeves-eval/
COPY runtime/crates/jeeves-loop/Cargo.toml ./runtime/crates/jeeves-loop/
COPY runtime/crates/jeeves-mcp/Cargo.toml ./runtime/crates/jeeves-mcp/
COPY runtime/crates/jeeves-memory/Cargo.toml ./runtime/crates/jeeves-memory/
COPY runtime/crates/jeeves-ops/Cargo.toml ./runtime/crates/jeeves-ops/
COPY protocol/Cargo.toml ./protocol/
COPY protocol/noop-macros/Cargo.toml ./protocol/noop-macros/
COPY runtime/crates/jeeves-provider/Cargo.toml ./runtime/crates/jeeves-provider/

COPY runtime/crates/jeeves-state/Cargo.toml ./runtime/crates/jeeves-state/
COPY runtime/crates/jeeves-tools/Cargo.toml ./runtime/crates/jeeves-tools/
# Every library crate gets an empty lib.rs and the CLI an empty main. Our own stub crates
# may fail to link against each other; `|| true` is fine because only the dependency
# artefacts are wanted from this step.
RUN for crate in runtime/crates/jeeves-*/ protocol/ protocol/noop-macros/; do mkdir -p "$crate/src" && : > "$crate/src/lib.rs"; done \
    && rm runtime/crates/jeeves-cli/src/lib.rs \
    && echo 'fn main() {}' > runtime/crates/jeeves-cli/src/main.rs \
    && cargo build --release --locked -p jeeves-cli || true
# Then the real sources on top. Touch them: COPY keeps the context's mtimes, which can be
# older than the stubs, and cargo would then keep the stub builds.
COPY runtime ./runtime
COPY protocol ./protocol
RUN find runtime protocol -path '*/src/*.rs' -exec touch {} + \
    && cargo build --release --locked -p jeeves-cli

FROM node:22-trixie-slim AS channels
WORKDIR /src
COPY package.json package-lock.json ./
COPY web/package.json ./web/
COPY executors/browser/package.json ./executors/browser/
COPY integrations/channels ./integrations/channels
RUN npm ci --omit=dev --workspace @jeeves/channels

FROM golang:1.26-bookworm AS relay
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY internal/cloudkit ./internal/cloudkit
COPY gateway ./gateway
RUN CGO_ENABLED=0 go build -trimpath -o /out/jeeves-desktop-relay ./gateway

# ---- Runtime ---------------------------------------------------------------
# Node 22 is here for the chat gateway the Runtime hosts (Settings -> Connections).
FROM node:22-trixie-slim
# bubblewrap = the Linux sandbox backend. run_command executes nothing without it (on
# purpose: never outside a sandbox), so the exec tools inside a container need it.
# ca-certificates for provider and MCP HTTPS.
RUN apt-get update \
    && apt-get install -y --no-install-recommends bubblewrap ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/target/release/jeeves /usr/local/bin/jeeves
COPY --from=relay /out/jeeves-desktop-relay /usr/local/bin/jeeves-desktop-relay
COPY --from=channels /src/integrations /opt/jeeves/integrations
COPY --from=channels /src/node_modules /opt/jeeves/node_modules
COPY protocol/schema/typescript /opt/jeeves/protocol/schema/typescript
COPY protocol/typescript /opt/jeeves/protocol/typescript
ENV JEEVES_GATEWAY_BIN=/opt/jeeves/integrations/channels/src/bin-lark.ts

# Run as non-root: nothing in the container needs privilege.
RUN useradd --create-home --shell /bin/bash jeeves
USER jeeves
# The private gateway and control plane reach this listener over the cluster network.
ENV JEEVES_OPS_HOST=0.0.0.0
ENV JEEVES_HOME=/home/jeeves/.jeeves
VOLUME /home/jeeves/.jeeves
EXPOSE 7777

ENTRYPOINT ["jeeves"]
CMD ["runtime"]
