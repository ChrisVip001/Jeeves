FROM node:22-trixie-slim AS web
WORKDIR /src
COPY jeeves-ts ./jeeves-ts
COPY jeeves-rs/jeeves-protocol/schema ./jeeves-rs/jeeves-protocol/schema
RUN cd jeeves-ts && npm ci && npm run build --workspace @jeeves/dashboard

FROM node:22-trixie-slim
WORKDIR /app
COPY control-plane/package.json ./
COPY control-plane/src ./src
COPY --from=web /src/jeeves-ts/packages/dashboard/dist ./dashboard
ENV CP_DASHBOARD_DIR=/app/dashboard
# kubectl provisions user runtimes. curl also provides authenticated SMTP over TLS.
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates \
    && curl -fsSLo /usr/local/bin/kubectl "https://dl.k8s.io/release/v1.31.0/bin/linux/$(dpkg --print-architecture)/kubectl" \
    && chmod +x /usr/local/bin/kubectl && rm -rf /var/lib/apt/lists/*
USER node
EXPOSE 8080
CMD ["node", "src/server.ts"]
