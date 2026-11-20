FROM node:22-trixie-slim
WORKDIR /app
COPY package.json ./
COPY src ./src
# kubectl is how the control plane talks to the cluster; pin it in your own build.
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates \
    && curl -fsSLo /usr/local/bin/kubectl "https://dl.k8s.io/release/v1.31.0/bin/linux/$(dpkg --print-architecture)/kubectl" \
    && chmod +x /usr/local/bin/kubectl && apt-get purge -y curl && rm -rf /var/lib/apt/lists/*
USER node
EXPOSE 8080
CMD ["node", "src/server.ts"]
