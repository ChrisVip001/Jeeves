import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import type { IncomingMessage, ServerResponse } from "node:http";
import { extname, join } from "node:path";

export const cookieName = "__Host-jeeves_session";

export function sessionCookie(token: string, maxAge: number) {
  return `${cookieName}=${token}; Path=/; Max-Age=${maxAge}; Secure; HttpOnly; SameSite=Lax`;
}

export function tokenFrom(req: IncomingMessage) {
  return req.headers.cookie
    ?.split(";")
    .map((c) => c.trim())
    .find((c) => c.startsWith(cookieName + "="))
    ?.slice(cookieName.length + 1);
}

export function clientLabel(req: IncomingMessage) {
  const ua = req.headers["user-agent"] ?? "";
  if (/iPhone|iPad/.test(ua)) return "iOS";
  return /Macintosh|Mac OS X/.test(ua) ? "Mac" : "Web";
}

export function json(
  res: ServerResponse,
  status: number,
  value: unknown,
  headers: Record<string, string> = {},
) {
  res.writeHead(status, { "content-type": "application/json", ...headers });
  res.end(JSON.stringify(value));
}

export function redirect(res: ServerResponse, location: string) {
  res.writeHead(303, { location });
  res.end();
}

export function validMutation(req: IncomingMessage, origin?: string) {
  if (req.headers["sec-fetch-site"] === "cross-site") return false;
  if (req.headers.origin)
    return req.headers.origin === (origin ?? `http://${req.headers.host}`);
  return (
    req.headers["content-type"]?.split(";")[0] === "application/json" ||
    req.headers["x-jeeves-client"] === "1"
  );
}

export async function accountFields(req: IncomingMessage) {
  let bytes = 0;
  const chunks: Buffer[] = [];
  for await (const chunk of req) {
    bytes += chunk.length;
    if (bytes > 16384) throw new Error("account request too large");
    chunks.push(chunk);
  }
  const b: unknown = JSON.parse(Buffer.concat(chunks).toString());
  if (!b || typeof b !== "object" || Array.isArray(b))
    throw new Error("invalid account request");
  return b as Record<string, unknown>;
}

const types: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript",
  ".css": "text/css",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".webmanifest": "application/manifest+json",
  ".woff2": "font/woff2",
};

/** Only the gateway's own immutable dashboard bundle can supply executable browser assets. */
export async function asset(
  req: IncomingMessage,
  res: ServerResponse,
  dir: string,
  path: string,
) {
  if (req.method !== "GET" && req.method !== "HEAD")
    return json(res, 405, { error: "method not allowed" });
  if (
    !/^[a-zA-Z0-9_./-]+$/.test(path) ||
    path.split("/").some((x) => !x || x === "." || x === "..") ||
    !types[extname(path)]
  )
    return json(res, 404, { error: "not found" });
  const file = join(dir, path);
  const info = await stat(file).catch(() => null);
  if (!info?.isFile())
    return json(res, 404, { error: "dashboard asset unavailable" });
  res.setHeader("content-type", types[extname(path)]);
  res.setHeader("content-length", info.size);
  res.setHeader(
    "content-security-policy",
    "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'; frame-ancestors 'none'; object-src 'none'; base-uri 'none'; form-action 'self'",
  );
  if (req.method === "HEAD") return res.end();
  const stream = createReadStream(file);
  res.on("close", () => stream.destroy());
  stream.on("error", () => res.destroy()).pipe(res);
}

/** A single control-plane writer handles this bounded window. Ingress can additionally limit by trusted client IP. */
export function authLimiter() {
  const attempts = new Map<string, { until: number; count: number }>();
  return (key: string, limit: number) => {
    const now = Date.now();
    for (const [k, record] of attempts) {
      if (record.until > now) break;
      attempts.delete(k);
    }
    const record = attempts.get(key);
    if (record) return ++record.count <= limit;
    if (attempts.size >= 16384) return false;
    attempts.set(key, { until: now + 60_000, count: 1 });
    return true;
  };
}
