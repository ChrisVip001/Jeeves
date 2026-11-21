import { createServer as createTlsServer } from "node:https";
import { readFileSync } from "node:fs";
import { createServer, type ServerResponse } from "node:http";
import { fileURLToPath } from "node:url";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import {
  accountFields,
  asset,
  authLimiter,
  json,
  redirect,
  sessionCookie,
  tokenFrom,
  validMutation,
} from "./web.ts";
import { identity } from "./identity.ts";
import type { AccountMailer } from "./mail.ts";
import { deployment } from "./deployment.ts";
import type { Accounts } from "./accounts.ts";
import type { Cluster } from "./cluster.ts";
import { podUrl, type UserPod } from "./manifests.ts";
import { handleModel, type ProxyConfig } from "./proxy.ts";

export interface Config {
  mail?: AccountMailer;
  image: string;
  namespace: string;
  /** In-cluster address of this control plane, as the Pods reach the model proxy. */
  publicModelUrl: string;
  idleMinutes: number;
  proxy: ProxyConfig;
  publicOrigin?: string;
  dashboardDir?: string;
  single?: { ownerId: string; registrationEmail?: string; token: string };
  /** Override for tests. */
  podAddress?: (id: string) => string;
}

export function podSpec(
  a: { id: string; opsToken: string; modelToken: string; awake: boolean },
  c: Config,
): UserPod {
  return {
    id: a.id,
    awake: a.awake,
    opsToken: a.opsToken,
    modelToken: a.modelToken,
    image: c.image,
    modelBaseUrl: c.publicModelUrl,
  };
}

async function healthy(url: string, tries = 60): Promise<boolean> {
  for (let i = 0; i < tries; i++) {
    try {
      if (
        (
          await fetch(`${url}/healthz`, {
            signal: AbortSignal.timeout(3000),
            redirect: "error",
          })
        ).ok
      )
        return true;
    } catch {}
    await new Promise((r) => setTimeout(r, 1000));
  }
  return false;
}

export function createControlPlane(
  accounts: Accounts,
  cluster: Cluster,
  cfg: Config,
) {
  const bundle =
    cfg.dashboardDir ??
    fileURLToPath(
      new URL("../../jeeves-ts/packages/dashboard/dist", import.meta.url),
    );
  const activeSessions = new Map<string, Set<ServerResponse>>();
  const allow = authLimiter();
  const address = cfg.podAddress ?? ((id: string) => podUrl(id, cfg.namespace));
  function revoke(accountId: string, sessionId: string) {
    if (!accounts.revoke(accountId, sessionId)) return false;
    for (const stream of activeSessions.get(sessionId) ?? []) stream.destroy();
    return true;
  }

  const waking = new Map<string, Promise<boolean>>();
  function wake(id: string): Promise<boolean> {
    const existing = waking.get(id);
    if (existing) return existing;
    const pending = (async () => {
      const account = accounts.byId(id)!;
      if (!account.awake)
        await cluster.apply(podSpec({ ...account, awake: true }, cfg));
      const ready = await healthy(address(id));
      if (ready) accounts.touch(id, true);
      return ready;
    })().finally(() => waking.delete(id));
    waking.set(id, pending);
    return pending;
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
      if (!response.ok)
        throw new Error(`task list returned ${response.status}`);
      const page = await response.json();
      if (!Array.isArray(page.tasks)) throw new Error("invalid task list");
      if (
        page.tasks.some(
          (task: { status: string }) =>
            !["done", "failed", "cancelled"].includes(task.status),
        )
      )
        return false;
      if (page.nextCursor == null) return true;
      if (typeof page.nextCursor !== "string" || visited.has(page.nextCursor))
        throw new Error("invalid task list cursor");
      visited.add(page.nextCursor);
      url.searchParams.set("cursor", page.nextCursor);
    }
  }

  async function sweep(now = Date.now()) {
    if (cfg.single) return;
    for (const account of accounts.all()) {
      const { id } = account;
      if (
        !account.awake ||
        account.alwaysOn ||
        activeRequests.has(id) ||
        sleeping.has(id) ||
        now - account.lastSeen <= cfg.idleMinutes * 60_000
      )
        continue;
      const pending = (async () => {
        if (
          !(await tasksSettled(id, account.opsToken)) ||
          accounts.byId(id) !== account
        )
          return;
        await cluster.scale(id, 0);
        accounts.touch(id, false);
        console.error(
          `(control-plane) ${id} put to sleep after ${cfg.idleMinutes} min idle`,
        );
      })();
      sleeping.set(id, pending);
      try {
        await pending;
      } catch (error) {
        console.error(`(control-plane) idle check for ${id}: ${error}`);
      } finally {
        sleeping.delete(id);
      }
    }
  }

  const server = createServer(async (req, res) => {
    try {
      res.setHeader("cache-control", "no-store");
      res.setHeader("x-content-type-options", "nosniff");
      res.setHeader("referrer-policy", "no-referrer");
      res.setHeader("x-frame-options", "DENY");
      if (cfg.publicOrigin)
        res.setHeader("strict-transport-security", "max-age=31536000");
      const target = new URL(req.url ?? "/", "http://gateway.invalid");
      const url = target.pathname;
      if (url === "/healthz") return json(res, 200, { ok: true });
      if (url.startsWith("/v1/") && !cfg.single)
        return await handleModel(req, res, accounts, cfg.proxy);
      if (
        cfg.publicOrigin &&
        (req.headers.host !== new URL(cfg.publicOrigin).host ||
          req.headers["x-forwarded-proto"] !== "https")
      )
        return json(res, 403, { error: "use the configured HTTPS gateway" });
      const read = req.method === "GET" || req.method === "HEAD";
      if (!read && !validMutation(req, cfg.publicOrigin))
        return json(res, 403, { error: "request origin rejected" });
      if (url === "/" || url === "/account") return redirect(res, "/account/");
      if (url.startsWith("/account/")) {
        const file = url.slice(9);
        if (!file || file === "index.html")
          return await asset(req, res, bundle, "account/account.html");
        if (
          file === "account.js" ||
          file === "icon.svg" ||
          /^(assets|chunks)\//.test(file)
        )
          return await asset(req, res, bundle, "account/" + file);
        return json(res, 404, { error: "not found" });
      }
      if (
        req.method === "POST" &&
        [
          "/api/login",
          "/api/register",
          "/api/forgot",
          "/api/reset",
          "/api/verify",
          "/api/send-verification",
          "/api/account/factor/start",
          "/api/account/factor/confirm",
          "/api/account/factor/disable",
        ].includes(url)
      ) {
        return await identity(req, res, url, accounts, {
          ...cfg,
          allow,
          close(sessions) {
            for (const { id } of sessions)
              for (const stream of activeSessions.get(id) ?? [])
                stream.destroy();
          },
          async provision(id) {
            if (cfg.single) {
              accounts.touch(id, true);
              return;
            }
            try {
              await cluster.apply(podSpec(accounts.byId(id)!, cfg));
            } catch {
              console.error(
                "(control-plane) initial provisioning deferred until wake",
              );
            }
          },
        });
      }
      const token = tokenFrom(req);
      const session = accounts.authenticate(token);
      const me = session && accounts.byId(session.accountId);
      if (
        !session ||
        !me?.verified ||
        (cfg.single && me.id !== cfg.single.ownerId)
      ) {
        if (
          read &&
          url.startsWith("/app/") &&
          req.headers.accept?.includes("text/html")
        )
          return redirect(
            res,
            "/account/?next=" +
              encodeURIComponent(target.pathname + target.search),
          );
        return json(
          res,
          401,
          {
            error: "sign in first",
            signup:
              !cfg.single ||
              Boolean(cfg.single.registrationEmail && !cfg.single.ownerId),
          },
          { "x-jeeves-session": "required" },
        );
      }
      if (url === "/api/account" && req.method === "GET")
        return json(res, 200, {
          email: me.email,
          factorEnabled: !!me.factor,
          createdAt: me.createdAt,
          app: `/app/${me.id}/`,
          used: accounts.usage(me.id),
          quota: cfg.single ? null : cfg.proxy.monthlyTokens,
          current: session.id,
          sessions: accounts.sessions(me.id),
        });
      if (url === "/api/account/password" && req.method === "POST") {
        if (!allow("password:" + me.id, 8))
          return json(
            res,
            429,
            { error: "try again later" },
            { "retry-after": "60" },
          );
        const fields = await accountFields(req).catch(() => null);
        if (
          typeof fields?.current !== "string" ||
          typeof fields.replacement !== "string" ||
          fields.replacement.length < 10
        )
          return json(res, 400, { error: "invalid password change" });
        if (!accounts.authenticate(token))
          return json(res, 401, { error: "sign in again" });
        const sessions = accounts.sessions(me.id);
        try {
          accounts.changePassword(me.id, fields.current, fields.replacement);
        } catch (error) {
          if ((error as Error).message === "wrong email or password")
            return json(res, 403, { error: "current password incorrect" });
          throw error;
        }
        for (const { id } of sessions)
          for (const stream of activeSessions.get(id) ?? []) stream.destroy();
        return json(
          res,
          200,
          { ok: true },
          { "set-cookie": sessionCookie("", 0) },
        );
      }
      const sessionRoute = /^\/api\/account\/sessions\/([a-f0-9]{32})$/.exec(
        url,
      );
      if (
        (req.method === "DELETE" && sessionRoute) ||
        (req.method === "POST" && url === "/api/logout")
      ) {
        const id = sessionRoute?.[1] ?? session.id;
        if (!revoke(me.id, id))
          return json(res, 404, { error: "session not found" });
        return json(
          res,
          200,
          { ok: true },
          id === session.id ? { "set-cookie": sessionCookie("", 0) } : {},
        );
      }
      if (url === "/app" || url === "/app/" || url === "/app/index.html")
        return redirect(res, `/app/${me.id}/${target.search}`);
      const app = /^\/app\/(u-[a-f0-9]{12})\/(.*)$/.exec(url);
      if (
        app &&
        app[1] !== me.id &&
        req.headers["sec-fetch-mode"] === "navigate"
      )
        return redirect(res, "/account/");
      if (app && app[1] !== me.id)
        return json(
          res,
          403,
          { error: "this workspace belongs to another account" },
          { "x-jeeves-session": "required" },
        );
      if (app && !app[2]!.startsWith("api/"))
        return await asset(req, res, bundle, app[2] || "index.html");
      const runtimePath = app
        ? "/" + app[2]
        : url.startsWith("/app/api/")
          ? url.slice(4)
          : undefined;
      if (runtimePath && !app && req.headers["sec-fetch-site"])
        return json(
          res,
          403,
          { error: "open the account workspace" },
          { "x-jeeves-session": "required" },
        );
      if (url === "/api/wake" || runtimePath) {
        const group =
          activeSessions.get(session.id) ?? new Set<ServerResponse>();
        activeSessions.set(session.id, group);
        group.add(res);
        const expired = setTimeout(
          () => res.destroy(),
          Math.max(1, session.expires - Date.now()),
        );
        const controller = new AbortController();
        res.once("close", () => {
          clearTimeout(expired);
          controller.abort();
          group.delete(res);
          if (!group.size) activeSessions.delete(session.id);
        });
        accounts.touch(me.id);
        const pending = sleeping.get(me.id);
        if (pending) await pending.catch(() => {});
        if (res.destroyed) return;
        if (req.method === "POST" && url === "/api/wake")
          return json(res, 200, {
            ready: await wake(me.id),
            used: accounts.usage(me.id),
          });
        if (!runtimePath)
          return json(res, 405, { error: "method not allowed" });
        if (!read) {
          activeRequests.set(me.id, (activeRequests.get(me.id) ?? 0) + 1);
          res.once("close", () => {
            const remaining = activeRequests.get(me.id)! - 1;
            if (remaining) activeRequests.set(me.id, remaining);
            else activeRequests.delete(me.id);
          });
        }
        if (!accounts.byId(me.id)!.awake && !(await wake(me.id)))
          return json(res, 503, { error: "Runtime is starting" });
        if (res.destroyed || !accounts.authenticate(token)) return;
        const upstream = await fetch(
          address(me.id) + runtimePath + target.search,
          {
            method: req.method,
            redirect: "error",
            signal: controller.signal,
            headers: {
              authorization: `Bearer ${cfg.single?.token ?? me.opsToken}`,
              "content-type": String(
                req.headers["content-type"] ?? "application/json",
              ),
              accept: String(req.headers.accept ?? "*/*"),
            },
            ...(read
              ? {}
              : ({ body: Readable.toWeb(req), duplex: "half" } as RequestInit)),
          },
        );
        res.setHeader("content-security-policy", "sandbox; default-src 'none'");
        if (
          upstream.headers.get("content-type")?.startsWith("text/event-stream")
        )
          res.setHeader("x-accel-buffering", "no");
        res.writeHead(upstream.status, {
          "content-type":
            upstream.headers.get("content-type") ?? "application/octet-stream",
        });
        res.flushHeaders();
        if (upstream.body)
          await pipeline(Readable.from(upstream.body.values()), res);
        else res.end();
        return;
      }
      return json(res, 404, { error: "not found" });
    } catch {
      if (res.destroyed) return;
      if (res.headersSent) {
        res.destroy();
        console.error(
          "(control-plane) upstream stream or response finalization failed",
        );
        return;
      }
      if (!res.destroyed)
        return json(res, 502, { error: "service temporarily unavailable" });
    }
  });
  return { server, sweep, wake };
}

if (import.meta.main) {
  const { accounts, cfg, cluster, port, bind } = deployment(process.env);
  const { server, sweep } = createControlPlane(accounts, cluster, cfg);
  setInterval(
    () =>
      void sweep().catch(() => console.error("(control-plane) sweep failed")),
    60_000,
  ).unref();
  const { CP_TLS_CERT, CP_TLS_KEY } = process.env;
  if (Boolean(CP_TLS_CERT) !== Boolean(CP_TLS_KEY))
    throw new Error("CP_TLS_CERT and CP_TLS_KEY must be set together");
  const listener =
    CP_TLS_CERT && CP_TLS_KEY
      ? createTlsServer(
          {
            cert: readFileSync(CP_TLS_CERT),
            key: readFileSync(CP_TLS_KEY),
          },
          (req, res) => {
            req.headers["x-forwarded-proto"] = "https";
            server.emit("request", req, res);
          },
        )
      : server;
  listener.listen(port, bind, () => console.error("(control-plane) listening"));
}
