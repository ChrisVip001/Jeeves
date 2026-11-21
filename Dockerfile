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
FROM rust:1.98-slim-trixie AS builder
WORKDIR /src
# Manifests first, with stub sources: the dependency layer is cached by Docker, so a code
# change does not rebuild every dependency.
COPY jeeves-rs/Cargo.toml jeeves-rs/Cargo.lock ./jeeves-rs/
COPY jeeves-rs/jeeves-agent/Cargo.toml ./jeeves-rs/jeeves-agent/
COPY jeeves-rs/jeeves-app-server/Cargo.toml ./jeeves-rs/jeeves-app-server/
COPY jeeves-rs/jeeves-cli/Cargo.toml ./jeeves-rs/jeeves-cli/
COPY jeeves-rs/jeeves-config/Cargo.toml ./jeeves-rs/jeeves-config/
COPY jeeves-rs/jeeves-connector/Cargo.toml ./jeeves-rs/jeeves-connector/
COPY jeeves-rs/jeeves-eval/Cargo.toml ./jeeves-rs/jeeves-eval/
COPY jeeves-rs/jeeves-loop/Cargo.toml ./jeeves-rs/jeeves-loop/
COPY jeeves-rs/jeeves-mcp/Cargo.toml ./jeeves-rs/jeeves-mcp/
COPY jeeves-rs/jeeves-memory/Cargo.toml ./jeeves-rs/jeeves-memory/
COPY jeeves-rs/jeeves-ops/Cargo.toml ./jeeves-rs/jeeves-ops/
COPY jeeves-rs/jeeves-protocol/Cargo.toml ./jeeves-rs/jeeves-protocol/
COPY jeeves-rs/jeeves-protocol-noop-macros/Cargo.toml ./jeeves-rs/jeeves-protocol-noop-macros/
COPY jeeves-rs/jeeves-provider/Cargo.toml ./jeeves-rs/jeeves-provider/

COPY jeeves-rs/jeeves-state/Cargo.toml ./jeeves-rs/jeeves-state/
COPY jeeves-rs/jeeves-tools/Cargo.toml ./jeeves-rs/jeeves-tools/
COPY jeeves-rs/rust-toolchain.toml ./jeeves-rs/
# Every library crate gets an empty lib.rs and the CLI an empty main. Our own stub crates
# may fail to link against each other; `|| true` is fine because only the dependency
# artefacts are wanted from this step.
RUN for crate in jeeves-rs/jeeves-*/; do mkdir -p "$crate/src" && : > "$crate/src/lib.rs"; done \
    && rm jeeves-rs/jeeves-cli/src/lib.rs \
    && echo 'fn main() {}' > jeeves-rs/jeeves-cli/src/main.rs \
    && cargo build --release --manifest-path jeeves-rs/Cargo.toml -p jeeves-cli || true
# Then the real sources on top. Touch them: COPY keeps the context's mtimes, which can be
# older than the stubs, and cargo would then keep the stub builds.
COPY jeeves-rs ./jeeves-rs
RUN find jeeves-rs -path '*/src/*.rs' -exec touch {} + \
    && cargo build --release --manifest-path jeeves-rs/Cargo.toml -p jeeves-cli

FROM node:22-trixie-slim AS channels
WORKDIR /src
COPY jeeves-ts ./jeeves-ts
RUN cd jeeves-ts && npm ci --omit=dev --workspace @jeeves/channels

FROM golang:1.26-bookworm AS relay
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY internal/cloudkit ./internal/cloudkit
COPY jeeves-ts/packages/gateway ./jeeves-ts/packages/gateway
RUN CGO_ENABLED=0 go build -trimpath -o /out/jeeves-desktop-relay ./jeeves-ts/packages/gateway

# ---- Runtime ---------------------------------------------------------------
# Node 22 is here for the chat gateway the Runtime hosts (Settings -> Connections).
FROM node:22-trixie-slim
# bubblewrap = the Linux sandbox backend. run_command executes nothing without it (on
# purpose: never outside a sandbox), so the exec tools inside a container need it.
# ca-certificates for provider and MCP HTTPS.
RUN apt-get update \
    && apt-get install -y --no-install-recommends bubblewrap ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/jeeves-rs/target/release/jeeves /usr/local/bin/jeeves
COPY --from=relay /out/jeeves-desktop-relay /usr/local/bin/jeeves-desktop-relay
COPY --from=channels /src/jeeves-ts /opt/jeeves/jeeves-ts
ENV JEEVES_GATEWAY_BIN=/opt/jeeves/jeeves-ts/packages/channels/src/bin-lark.ts

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
