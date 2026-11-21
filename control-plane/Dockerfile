FROM golang:1.26-bookworm AS build
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY internal/cloudkit ./internal/cloudkit
COPY control-plane/*.go ./control-plane/
RUN CGO_ENABLED=0 go build -trimpath -o /out/jeeves-control-plane ./control-plane

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /out/jeeves-control-plane /usr/local/bin/jeeves-control-plane
USER 1000:1000
EXPOSE 8080
ENTRYPOINT ["jeeves-control-plane"]
