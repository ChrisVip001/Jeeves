// Jeeves's service worker: network first for pages and styles, cache first for the rest of the
// static assets, and /api always straight through.
//
// Three rules:
// 1. /api/* is never cached, not one frame — conversations, memory and approvals are live; the
//    server already sends no-store, and this goes straight through too.
// 2. index.html / style.css are network first: cache first produces «fixed it, but the browser
//    still shows the old one», most visibly on a layout fix. Only when the network fails does it
//    fall back to the cache, so offline still opens.
// 3. Every other static asset is cache first with a background update; bump the version and the
//    whole old cache is void.
const scope = self.registration.scope;
const cachePrefix = `jeeves:${scope}:`;
// copy-assets stamps the same entry as index.html so a first install can boot offline.
const ENTRY = "./main.js";
const VERSION = `${cachePrefix}v45:${ENTRY}`;
const CORE = [ENTRY, "./", "./index.html", "./style.css", "./icon.svg", "./manifest.webmanifest"];

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches.open(VERSION).then((cache) => cache.addAll(CORE)).then(() => self.skipWaiting()),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((key) => key !== VERSION && (key.startsWith(cachePrefix) || /^jeeves-v\d+$/.test(key))).map((key) => caches.delete(key))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  if (event.request.method !== "GET" || !url.href.startsWith(scope)) return;
  const file = url.pathname.slice(new URL(scope).pathname.length);
  const approvalEntry = file === "" && url.searchParams.has("approvalRequestId");
  if (event.request.mode === "navigate" && !approvalEntry) return;
  if (!["", "index.html", "style.css", "main.js", "icon.svg", "icon-maskable.svg", "manifest.webmanifest"].includes(file)
      && !file.startsWith("assets/") && !file.startsWith("chunks/")) return;

  // Navigation requests and styles have to hit the network first: one refresh has to bring the new
  // layout, not wait for a background update to swap it in quietly.
  const pageLike = url.pathname.endsWith("/") || /\.(?:html|css)$/.test(url.pathname);
  event.respondWith((async () => {
    const cache = await caches.open(VERSION).catch(() => undefined);
    if (!cache) return fetch(event.request);
    const cacheKey = approvalEntry ? scope : event.request;
    const cached = await cache.match(cacheKey).catch(() => undefined);
    const fresh = fetch(event.request).then(async (response) => {
      if (response.ok && !response.redirected && new URL(response.url).origin === self.location.origin) {
        await cache.put(cacheKey, response.clone()).catch(() => {});
      }
      return response;
    }).catch((error) => {
      if (cached) return cached;
      throw error;
    });
    if (!pageLike && cached) {
      event.waitUntil(fresh);
      return cached;
    }
    return fresh;
  })());
});

// A notification contains only identifiers. The cockpit resolves them against live state.
function noticeLink(notice) {
  if (!notice || typeof notice !== "object") return null;
  const url = new URL(scope);
  for (const key of ["taskId", "turnId", "approvalRequestId", "deviceId", "grantId"]) {
    if (typeof notice[key] !== "string" || !notice[key] || notice[key].length > 256) return null;
    url.searchParams.set(key, notice[key]);
  }
  for (const key of ["approvalVersion", "generation"]) {
    if (!Number.isInteger(notice[key]) || notice[key] < 1 || notice[key] > 2147483647) return null;
    url.searchParams.set(key, String(notice[key]));
  }
  return url;
}
self.addEventListener("push", (event) => {
  let notice;
  try { notice = event.data?.json(); } catch { return; }
  const link = noticeLink(notice);
  if (!link) return;
  const chinese = self.navigator.language.toLowerCase().startsWith("zh");
  event.waitUntil(self.registration.showNotification(chinese ? "Jeeves · 有一项审批待查看" : "Jeeves · Approval requested", {
    body: chinese ? "打开原任务查看操作范围。" : "Open the task to review the requested action.",
    tag: link.search, data: Object.fromEntries([...link.searchParams].map(([key]) => [key, notice[key]])),
  }));
});
self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const link = noticeLink(event.notification.data);
  if (link) event.waitUntil(self.clients.openWindow(link.href));
});
