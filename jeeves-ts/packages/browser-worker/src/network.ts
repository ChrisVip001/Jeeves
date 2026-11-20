import http from "node:http";
import net from "node:net";
import dns from "node:dns/promises";

const denied = new net.BlockList();
for (const [ip, bits] of [
  ["0.0.0.0", 8],
  ["10.0.0.0", 8],
  ["100.64.0.0", 10],
  ["127.0.0.0", 8],
  ["169.254.0.0", 16],
  ["172.16.0.0", 12],
  ["192.0.0.0", 24],
  ["192.0.2.0", 24],
  ["192.88.99.0", 24],
  ["192.168.0.0", 16],
  ["198.18.0.0", 15],
  ["198.51.100.0", 24],
  ["203.0.113.0", 24],
  ["224.0.0.0", 4],
  ["240.0.0.0", 4],
] as const)
  denied.addSubnet(ip, bits, "ipv4");
const globalV6 = new net.BlockList();
globalV6.addSubnet("2000::", 3, "ipv6");
const deniedV6 = new net.BlockList();
deniedV6.addSubnet("2001::", 23, "ipv6");
deniedV6.addSubnet("2001:db8::", 32, "ipv6");
deniedV6.addSubnet("2002::", 16, "ipv6");
deniedV6.addSubnet("3fff::", 20, "ipv6");
export function publicAddress(address: string): boolean {
  return net.isIP(address) === 4
    ? !denied.check(address, "ipv4")
    : net.isIP(address) === 6 &&
        globalV6.check(address, "ipv6") &&
        !deniedV6.check(address, "ipv6");
}
const loopback = (ip: string) => /^127\./.test(ip) || ip === "::1";
export class NetworkScope {
  readonly origins = new Set<string>();
  readonly blocked = new Set<string>();
  readonly allowLocalhost: boolean;
  constructor(origins: string[], allowLocalhost: boolean) {
    this.allowLocalhost = allowLocalhost;
    for (const origin of origins) this.add(origin);
  }
  add(origin: string) {
    const url = new URL(origin);
    if (
      !["http:", "https:"].includes(url.protocol) ||
      url.username ||
      url.password ||
      (this.origins.size >= 12 && !this.origins.has(url.origin))
    )
      throw Error("Invalid or excessive browser origins");
    if (url.pathname !== "/" || url.search || url.hash)
      throw Error("Scope entries must be origins, without paths");
    this.origins.add(url.origin);
  }
  permit(value: string): URL {
    const url = new URL(value);
    if (
      !["http:", "https:"].includes(url.protocol) ||
      url.username ||
      url.password ||
      !this.origins.has(url.origin)
    ) {
      if (this.blocked.size < 16) this.blocked.add(url.origin);
      throw Error("Browser destination is outside the human-approved scope");
    }
    return url;
  }
  async destination(url: URL) {
    const host = url.hostname.replace(/^\[|\]$/g, "");
    const answers = net.isIP(host)
      ? [{ address: host, family: net.isIP(host) }]
      : await Promise.race([
          dns.lookup(host, { all: true }),
          new Promise<never>((_, reject) => {
            const timer = setTimeout(
              () => reject(Error("DNS deadline exceeded")),
              5000,
            );
            timer.unref();
          }),
        ]);
    const local =
      this.allowLocalhost &&
      ["localhost", "127.0.0.1", "::1"].includes(host.toLowerCase());
    if (
      !answers.length ||
      answers.some(
        (item) =>
          !publicAddress(item.address) && !(local && loopback(item.address)),
      )
    )
      throw Error("Private, metadata and reserved destinations are blocked");
    return answers[0]!;
  }
}
/** Pin connections to the checked IP; a second DNS resolution cannot rebind Chromium to a private host. */
export async function proxy(scope: NetworkScope) {
  const sockets = new Set<net.Socket>();
  const server = http.createServer(
    { maxHeaderSize: 8192 },
    (request, response) => {
      void (async () => {
        const url = scope.permit(request.url || "");
        if (url.protocol !== "http:") throw Error("Use CONNECT for TLS");
        const target = await scope.destination(url);
        const headers: http.OutgoingHttpHeaders = {
          ...request.headers,
          host: url.host,
        };
        delete headers["proxy-authorization"];
        delete headers["proxy-connection"];
        const outgoing = http.request(
          {
            host: target.address,
            family: target.family,
            port: Number(url.port || 80),
            method: request.method,
            path: url.pathname + url.search,
            headers,
            timeout: 15000,
          },
          (incoming) => {
            response.writeHead(incoming.statusCode || 502, incoming.headers);
            incoming.pipe(response);
          },
        );
        outgoing.on("timeout", () => outgoing.destroy());
        outgoing.on("error", () => {
          if (!response.headersSent) response.writeHead(502);
          response.end();
        });
        request.on("aborted", () => outgoing.destroy());
        request.pipe(outgoing);
      })().catch(() => {
        response.writeHead(403);
        response.end("Destination refused by Jeeves browser scope");
      });
    },
  );
  server.on("connect", (request, socket, head) => {
    void (async () => {
      const url = scope.permit(`https://${request.url}/`);
      const target = await scope.destination(url);
      const upstream = net.connect({
        host: target.address,
        family: target.family,
        port: Number(url.port || 443),
      });
      sockets.add(upstream);
      upstream.on("close", () => sockets.delete(upstream));
      upstream.setTimeout(15000, () => upstream.destroy());
      upstream.once("connect", () => {
        socket.write("HTTP/1.1 200 Connection Established\r\n\r\n");
        if (head.length) upstream.write(head);
        socket.pipe(upstream);
        upstream.pipe(socket);
      });
      upstream.on("error", () => socket.destroy());
      socket.on("error", () => upstream.destroy());
      socket.on("close", () => upstream.destroy());
    })().catch(() => {
      socket.end("HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n");
    });
  });
  server.on("upgrade", (_request, socket) => socket.destroy());
  server.maxConnections = 64;
  server.requestTimeout = 20000;
  server.headersTimeout = 10000;
  server.on("connection", (socket) => {
    sockets.add(socket);
    socket.on("close", () => sockets.delete(socket));
  });
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  if (!address || typeof address === "string")
    throw Error("Browser proxy unavailable");
  return {
    url: `http://127.0.0.1:${address.port}`,
    close: async () => {
      for (const socket of sockets) socket.destroy();
      await new Promise<void>((resolve) => server.close(() => resolve()));
    },
  };
}
