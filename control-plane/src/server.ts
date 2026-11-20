
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { Accounts } from "./accounts.ts";
import { Kubectl, type Cluster } from "./cluster.ts";
import { podUrl, type UserPod } from "./manifests.ts";
import { handleModel, type ProxyConfig } from "./proxy.ts";

export interface Config {
  image: string;
  namespace: string;
  /** In-cluster address of this control plane, as the Pods reach the model proxy. */
  publicModelUrl: string;
  idleMinutes: number;
  proxy: ProxyConfig;
  /** Override for tests. */
  podAddress?: (id: string) => string;
}

export function podSpec(a: { id: string; opsToken: string; modelToken: string; awake: boolean }, c: Config): UserPod {
  return { id: a.id, awake: a.awake, opsToken: a.opsToken, modelToken: a.modelToken, image: c.image, modelBaseUrl: c.publicModelUrl };
}

function cookieOf(req: IncomingMessage): string | undefined {
  return /(?:^|;\s*)jeeves_session=([^;]+)/.exec(req.headers.cookie ?? "")?.[1];
}

async function body(req: IncomingMessage): Promise<{email: string; password: string}> {
  const chunks: Buffer[] = [];
  for await (const c of req) chunks.push(c as Buffer);
  let value: unknown;
  try {
    value = JSON.parse(Buffer.concat(chunks).toString());
  } catch {
    throw new Error("account request must contain valid JSON");
  }
  if (!value || typeof value !== "object" || !("email" in value) || typeof value.email !== "string" ||
      !("password" in value) || typeof value.password !== "string") {
    throw new Error("email and password must be strings");
  }
  return { email: value.email, password: value.password };
}

function json(res: ServerResponse, status: number, value: unknown, headers: Record<string, string> = {}) {
  res.writeHead(status, { "content-type": "application/json", ...headers });
  res.end(JSON.stringify(value));
}

async function healthy(url: string, tries = 60): Promise<boolean> {
  for (let i = 0; i < tries; i++) {
    try {
      if ((await fetch(`${url}/healthz`)).ok) return true;
    } catch {
    }
    await new Promise((r) => setTimeout(r, 1000));
  }
  return false;
}

export function createControlPlane(accounts: Accounts, cluster: Cluster, cfg: Config) {
  const address = cfg.podAddress ?? ((id: string) => podUrl(id, cfg.namespace));

  async function wake(id: string): Promise<boolean> {
    const a = accounts.byId(id)!;
    if (!a.awake) {
      await cluster.apply(podSpec({ ...a, awake: true }, cfg));
      accounts.touch(id, true);
    }
    return healthy(address(id));
  }

  const sleeping = new Map<string, Promise<void>>();
  const activeRequests = new Map<string, number>();

  async function tasksSettled(id: string, token: string): Promise<boolean> {
    const url = new URL("/api/tasks", address(id));
    const visited = new Set<string>();
    for (;;) {
      const response = await fetch(url, {
        headers: { authorization: `Bearer ${token}` },
        signal: AbortSignal.timeout(5000),
      });
      if (!response.ok) throw new Error(`task list returned ${response.status}`);
      const page = await response.json();
      if (!Array.isArray(page.tasks)) throw new Error("invalid task list");
      if (page.tasks.some((task: { status: string }) => !["done", "failed", "cancelled"].includes(task.status))) return false;
      if (page.nextCursor == null) return true;
      if (typeof page.nextCursor !== "string" || visited.has(page.nextCursor)) throw new Error("invalid task list cursor");
      visited.add(page.nextCursor);
      url.searchParams.set("cursor", page.nextCursor);
    }
  }

  async function sweep(now = Date.now()) {
    for (const account of accounts.all()) {
      const { id } = account;
      if (!account.awake || account.alwaysOn || activeRequests.has(id) || sleeping.has(id) ||
          now - account.lastSeen <= cfg.idleMinutes * 60_000) continue;
      const pending = (async () => {
        if (!await tasksSettled(id, account.opsToken) || accounts.byId(id) !== account) return;
        await cluster.scale(id, 0);
        accounts.touch(id, false);
        console.error(`(control-plane) ${id} put to sleep after ${cfg.idleMinutes} min idle`);
      })();
      sleeping.set(id, pending);
      try { await pending; }
      catch (error) { console.error(`(control-plane) idle check for ${id}: ${error}`); }
      finally { sleeping.delete(id); }
    }
  }

  const server = createServer(async (req, res) => {
    try {
      const url = req.url ?? "/";
      if (url === "/healthz") return json(res, 200, { ok: true });
      if (url.startsWith("/v1/")) return await handleModel(req, res, accounts, cfg.proxy);
      if (req.method === "POST" && (url === "/api/register" || url === "/api/login")) {
        const b = await body(req);
        const account =
          url === "/api/register"
            ? accounts.register(b.email, b.password)
            : accounts.login(b.email, b.password);
        if (url === "/api/register") await cluster.apply(podSpec(account, cfg));
        return json(res, 200, { id: account.id }, {
          "set-cookie": `jeeves_session=${accounts.session(account.id)}; HttpOnly; SameSite=Lax; Path=/`,
        });
      }
      const me = accounts.fromSession(cookieOf(req));
      if (!me) return json(res, 401, { error: "sign in first" });
      if (url === "/api/wake" || url.startsWith("/app/")) {
        accounts.touch(me.id);
        const pending = sleeping.get(me.id);
        if (pending) await pending.catch(() => {});
      }
      if (req.method === "POST" && url === "/api/wake") {
        return json(res, 200, { ready: await wake(me.id), used: accounts.usage(me.id) });
      }
      if (url.startsWith("/app/")) {
        if (req.method !== "GET" && req.method !== "HEAD") {
          activeRequests.set(me.id, (activeRequests.get(me.id) ?? 0) + 1);
          res.once("close", () => {
            const remaining = activeRequests.get(me.id)! - 1;
            if (remaining) activeRequests.set(me.id, remaining);
            else activeRequests.delete(me.id);
          });
        }
        if (!accounts.byId(me.id)!.awake && !(await wake(me.id))) return json(res, 503, { error: "your Jeeves is still starting; retry in a moment" });
        const chunks: Buffer[] = [];
        for await (const c of req) chunks.push(c as Buffer);
        const upstream = await fetch(address(me.id) + url.slice(4), {
          method: req.method,
          headers: {
            authorization: `Bearer ${me.opsToken}`,
            "content-type": String(req.headers["content-type"] ?? "application/json"),
            accept: String(req.headers.accept ?? "*/*"),
          },
          body: ["GET", "HEAD"].includes(req.method ?? "") ? undefined : Buffer.concat(chunks),
        });
        res.writeHead(upstream.status, { "content-type": upstream.headers.get("content-type") ?? "application/octet-stream" });
        if (upstream.body) for await (const c of upstream.body as unknown as AsyncIterable<Uint8Array>) res.write(c);
        return res.end();
      }
      return json(res, 404, { error: "not found" });
    } catch (e) {
      if (res.headersSent) {
        res.destroy();
        console.error("(control-plane) upstream stream or response finalization failed");
        return;
      }
      return json(res, 400, { error: (e as Error).message });
    }
  });
  return { server, sweep, wake };
}

if (import.meta.main) {
  const env = process.env;
  const need = (k: string) => env[k] ?? (console.error(`${k} is required`), process.exit(2));
  const accounts = new Accounts(env.CP_STATE ?? "control-plane.json", need("CP_SESSION_SECRET"));
  const cfg: Config = {
    image: need("CP_IMAGE"),
    namespace: env.CP_NAMESPACE ?? "jeeves",
    publicModelUrl: need("CP_MODEL_URL"),
    idleMinutes: Number(env.CP_IDLE_MINUTES ?? 20),
    proxy: {
      upstream: need("CP_MODEL_UPSTREAM"),
      key: need("CP_MODEL_KEY"),
      monthlyTokens: Number(env.CP_MONTHLY_TOKENS ?? 2_000_000),
    },
  };
  const { server, sweep } = createControlPlane(accounts, new Kubectl(cfg.namespace), cfg);
  setInterval(() => void sweep().catch((e) => console.error(`(control-plane) sweep failed: ${e}`)), 60_000);
  server.listen(Number(env.CP_PORT ?? 8080), () => console.error("(control-plane) listening"));
}
