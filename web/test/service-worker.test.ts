import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { runInNewContext } from "node:vm";

test("the proxied cockpit bypasses live API reads and preserves other apps' caches", async () => {
  const scope = "https://fixture.test/app/";
  const listeners = new Map<string, (event: unknown) => void>();
  const deleted: string[] = [];
  runInNewContext(readFileSync(new URL("../src/sw.js", import.meta.url), "utf8"), {
    URL,
    self: { registration: { scope }, location: new URL(scope), clients: { claim() {} },
      addEventListener(name: string, f: (event: unknown) => void) { listeners.set(name, f); } },
    caches: { keys: async () => ["jeeves-v35", "other-app"], delete: async (key: string) => deleted.push(key),
      open: async () => ({ match: async () => undefined }) },
    fetch: async () => ({ ok: false }),
  });
  let activation: Promise<void> | undefined;
  listeners.get("activate")!({ waitUntil(work: Promise<void>) { activation = work; } });
  await activation;
  assert.ok(!deleted.includes("other-app"));
  let intercepted = false;
  listeners.get("fetch")!({ request: { method: "GET", url: scope + "api/session/main" },
    respondWith() { intercepted = true; } });
  assert.equal(intercepted, false);
});

test("an unavailable static cache still serves the network response", async () => {
  const scope = "https://fixture.test/app/";
  const listeners = new Map<string, (event: unknown) => void>();
  const response = { ok: true, url: scope + "index.html" };
  runInNewContext(readFileSync(new URL("../src/sw.js", import.meta.url), "utf8"), {
    URL, self: { registration: { scope }, location: new URL(scope),
      addEventListener(name: string, f: (event: unknown) => void) { listeners.set(name, f); } },
    caches: { async open() { throw Error("fixture cache unavailable"); } },
    fetch: async () => response,
  });
  let reply: Promise<unknown> | undefined;
  listeners.get("fetch")!({ request: { method: "GET", url: response.url },
    respondWith(result: Promise<unknown>) { reply = result; } });
  assert.equal(await reply, response);
});

test("a push is only a deduplicated review link scoped to the cockpit", async () => {
  const scope = "https://fixture.test/app/";
  const listeners = new Map<string, (event: unknown) => void>();
  const shown: {tag: string; data: Record<string, string>}[] = [];
  const opened: string[] = [];
  runInNewContext(readFileSync(new URL("../src/sw.js", import.meta.url), "utf8"), {
    URL, self: { registration: { scope, async showNotification(_title: string, options: typeof shown[number]) { shown.push(options); } },
      navigator: {language: "en"}, clients: { async openWindow(url: string) {opened.push(url);} },
      addEventListener(name: string, cb: (ev: unknown) => void) {listeners.set(name, cb);} },
  });
  const notice = {taskId:"task", turnId:"turn", approvalRequestId:"ask", approvalVersion:2, grantId:"grant", generation:3, deviceId:"device", target:"https://elsewhere.invalid"};
  let work: Promise<unknown> | undefined;
  const waitUntil = (p: Promise<unknown>) => { work = p; };
  for (let i=0; i<2; i++) {
    listeners.get("push")!({data:{json:()=>notice},waitUntil}); await work;
  }
  assert.equal(shown.length,2);
  assert.equal(shown[0].tag,shown[1].tag);
  assert.equal(opened.length,0);
  listeners.get("notificationclick")!({notification:{data:shown[0].data,close(){}},waitUntil});
  await work;
  const url=new URL(opened[0]);
  assert.equal(url.origin+url.pathname,scope);
  assert.equal(url.searchParams.get("approvalRequestId"),"ask");
  assert.equal(url.searchParams.get("generation"),"3");
  assert.equal(url.searchParams.has("target"),false);
});
