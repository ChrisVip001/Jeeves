FROM node:22-bookworm-slim AS web
WORKDIR /app/jeeves-ts
COPY jeeves-ts/ ./
COPY jeeves-rs/jeeves-protocol/schema/ /app/jeeves-rs/jeeves-protocol/schema/
RUN npm ci && npm run build --workspace @jeeves/dashboard

FROM golang:1.26-bookworm AS build
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY internal/cloudkit ./internal/cloudkit
COPY jeeves-ts/packages/gateway ./jeeves-ts/packages/gateway
RUN CGO_ENABLED=0 go build -trimpath -o /out/jeeves-gateway ./jeeves-ts/packages/gateway

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /out/jeeves-gateway /usr/local/bin/jeeves-gateway
COPY --from=web /app/jeeves-ts/packages/dashboard/dist /app/dashboard
ENV GW_DASHBOARD_DIR=/app/dashboard
USER 1000:1000
EXPOSE 8080
ENTRYPOINT ["jeeves-gateway"]
