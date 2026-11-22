FROM node:22-bookworm-slim AS web
WORKDIR /app
COPY package.json package-lock.json tsconfig.base.json ./
COPY executors/browser/package.json ./executors/browser/
COPY integrations/channels/package.json ./integrations/channels/
COPY integrations/channels/runtime-client/package.json ./integrations/channels/runtime-client/
COPY web/ ./web/
COPY protocol/schema/typescript/ ./protocol/schema/typescript/
COPY protocol/typescript/ ./protocol/typescript/
RUN npm ci && npm run build --workspace @jeeves/dashboard

FROM golang:1.26-bookworm AS build
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY internal/cloudkit ./internal/cloudkit
COPY gateway ./gateway
RUN CGO_ENABLED=0 go build -trimpath -o /out/jeeves-gateway ./gateway

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /out/jeeves-gateway /usr/local/bin/jeeves-gateway
COPY --from=web /app/web/dist /app/dashboard
ENV GW_DASHBOARD_DIR=/app/dashboard
USER 1000:1000
EXPOSE 8080
ENTRYPOINT ["jeeves-gateway"]
