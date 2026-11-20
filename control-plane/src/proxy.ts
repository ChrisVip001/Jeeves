
import type { IncomingMessage, ServerResponse } from "node:http";
import type { Accounts } from "./accounts.ts";

export interface ProxyConfig {
  upstream: string;
  key: string;
  /** Tokens per account per month through the platform quota. */
  monthlyTokens: number;
}

export function tokenOf(req: IncomingMessage): string | undefined {
  const auth = req.headers.authorization;
  if (auth?.startsWith("Bearer ")) return auth.slice(7);
  const key = req.headers["x-api-key"];
  return typeof key === "string" ? key : undefined;
}

/** Running tally of usage fields seen in a JSON or SSE stream (input is sent first, output last). */
export class Tally {
  input = 0;
  output = 0;
  #pending = "";
  #streaming: boolean;
  constructor(streaming = true) { this.#streaming = streaming; }
  feed(chunk: string) {
    this.#pending += chunk;
    if (!this.#streaming) return;
    const lines = this.#pending.split("\n");
    this.#pending = lines.pop()!;
    for (const line of lines) this.#record(line);
  }
  #record(raw: string) {
    if (this.#streaming && !raw.startsWith("data:")) return;
    const payload = this.#streaming ? raw.slice(5).trim() : raw;
    if (!payload || payload === "[DONE]") return;
    let value;
    try { value = JSON.parse(payload); } catch { return; }
    const usage = value?.usage ?? value?.message?.usage ?? value?.response?.usage;
    if (!usage) return;
    const input = usage.input_tokens ?? usage.prompt_tokens;
    const output = usage.output_tokens ?? usage.completion_tokens;
    if (Number.isSafeInteger(input)) this.input = Math.max(this.input, input);
    if (Number.isSafeInteger(output)) this.output = Math.max(this.output, output);
  }
  get total() {
    this.#record(this.#pending);
    return this.input + this.output;
  }
}

export async function handleModel(
  req: IncomingMessage,
  res: ServerResponse,
  accounts: Accounts,
  cfg: ProxyConfig,
): Promise<void> {
  const token = tokenOf(req);
  const account = token ? accounts.byModelToken(token) : undefined;
  if (!account) return end(res, 401, "unknown model token");
  if (accounts.usage(account.id) >= cfg.monthlyTokens) {
    return end(res, 429, "this month's included model quota is used up; add your own model key in Settings → Models");
  }
  const chunks: Buffer[] = [];
  for await (const c of req) chunks.push(c as Buffer);
  const headers: Record<string, string> = { "content-type": String(req.headers["content-type"] ?? "application/json") };
  for (const h of ["anthropic-version", "anthropic-beta", "accept"]) {
    const v = req.headers[h];
    if (typeof v === "string") headers[h] = v;
  }
  headers.authorization = `Bearer ${cfg.key}`;
  headers["x-api-key"] = cfg.key;
  let upstream: Response;
  try {
    upstream = await fetch(cfg.upstream + (req.url ?? ""), {
      method: req.method,
      headers,
      body: ["GET", "HEAD"].includes(req.method ?? "") ? undefined : Buffer.concat(chunks),
    });
  } catch (e) {
    return end(res, 502, `model upstream unreachable: ${(e as Error).message}`);
  }
  res.writeHead(upstream.status, { "content-type": upstream.headers.get("content-type") ?? "application/json" });
  const tally = new Tally((upstream.headers.get("content-type") ?? "").includes("text/event-stream"));
  const decoder = new TextDecoder();
  if (upstream.body) {
    for await (const chunk of upstream.body as unknown as AsyncIterable<Uint8Array>) {
      res.write(chunk);
      tally.feed(decoder.decode(chunk, { stream: true }));
    }
  }
  res.end();
  tally.feed(decoder.decode());
  const total = tally.total;
  if (total > 0) accounts.addUsage(account.id, total);
}

function end(res: ServerResponse, status: number, message: string) {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify({ error: { message } }));
}
