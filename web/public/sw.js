// Service worker for the hetzner-monitor PWA (ADR-0015, ADR-0016, ADR-0017). Plain JavaScript,
// no build step. It shows Web Push notifications (end of this file) and caches the app shell only, so the UI opens offline and installs as a PWA. API data is never
// cached: requests under /api/ always go to the network untouched (sessions, CSRF and alert state
// must be live).
//
// Strategies:
// - navigations (any route, the server answers with index.html): network first, offline fallback
//   to the cached shell "/";
// - /assets/* (content-hashed by Vite, immutable): cache first, then network (and stored);
// - other same-origin GETs (manifest, icons): network first, fall back to the cache.
//
// Whenever a shell is stored (install, and every online navigation), the /assets/ files it
// references are stored with it: its scripts and stylesheets, and the fonts those stylesheets
// load. The first visit fetches them before this worker exists, so without this an offline start
// would get the shell without its bundle. The same step deletes /assets/ files the current shell
// no longer references, so old builds do not pile up in the cache.
//
// Bump CACHE when the caching scheme changes; activate drops every other "hm-shell-*" cache.

const CACHE = "hm-shell-v1";
const CACHE_PREFIX = "hm-shell-";
const SHELL = "/";
const PRECACHE = [
  SHELL,
  "/manifest.webmanifest",
  "/icons/icon.svg",
  "/icons/icon-192.png",
  "/icons/icon-512.png",
];

self.addEventListener("install", (event) => {
  event.waitUntil(
    (async () => {
      const cache = await caches.open(CACHE);
      // "reload" bypasses the HTTP cache so the shell is fresh.
      await cache.addAll(PRECACHE.map((url) => new Request(url, { cache: "reload" })));
      await syncShellAssets(cache);
      await self.skipWaiting();
    })(),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    (async () => {
      const names = await caches.keys();
      await Promise.all(
        names
          .filter((name) => name.startsWith(CACHE_PREFIX) && name !== CACHE)
          .map((name) => caches.delete(name)),
      );
      await self.clients.claim();
    })(),
  );
});

self.addEventListener("fetch", (event) => {
  const request = event.request;
  if (request.method !== "GET") return;

  const url = new URL(request.url);
  if (url.origin !== self.location.origin) return;
  // Never touch the API: no caching, no fallback, straight to the network.
  if (url.pathname === "/api" || url.pathname.startsWith("/api/")) return;

  if (request.mode === "navigate") {
    event.respondWith(navigation(event));
  } else if (url.pathname.startsWith("/assets/")) {
    event.respondWith(immutableAsset(request));
  } else {
    event.respondWith(networkFirst(request));
  }
});

/** True for a response worth storing: a plain same-origin 200 that was not redirected. */
function cacheable(response) {
  return response.ok && response.type === "basic" && !response.redirected;
}

async function navigation(event) {
  const request = event.request;
  try {
    const response = await fetch(request);
    const type = response.headers.get("Content-Type") ?? "";
    if (cacheable(response) && type.startsWith("text/html")) {
      // Every route serves the same index.html; keep the latest copy as the offline shell.
      const copy = response.clone();
      event.waitUntil(
        (async () => {
          const cache = await caches.open(CACHE);
          await cache.put(SHELL, copy);
          await syncShellAssets(cache);
        })().catch(() => {}),
      );
    }
    return response;
  } catch (error) {
    const shell = await caches.match(SHELL, { cacheName: CACHE });
    if (shell) return shell;
    throw error;
  }
}

// src="/assets/…" and href="/assets/…" in the shell, url(/assets/…) in a stylesheet.
const ASSET_REF = /(?:src|href)="(\/assets\/[^"]+)"|url\(\s*["']?(\/assets\/[^"')\s]+)/g;

/** The /assets/ paths a shell or stylesheet text references. */
function assetRefs(text) {
  return Array.from(text.matchAll(ASSET_REF), (match) => match[1] ?? match[2]);
}

/**
 * Makes the cached /assets/ files match the cached shell: stores what it references (scripts,
 * stylesheets and the fonts the stylesheets load), then deletes the files of older builds.
 * Files that cannot be fetched are skipped; they are stored later on first use.
 */
async function syncShellAssets(cache) {
  const shell = await cache.match(SHELL);
  if (!shell) return;
  const wanted = new Set(assetRefs(await shell.text()));
  for (const path of Array.from(wanted)) {
    if (!path.endsWith(".css")) continue;
    const stylesheet = await ensureCached(cache, path);
    if (stylesheet) for (const ref of assetRefs(await stylesheet.text())) wanted.add(ref);
  }
  await Promise.all(Array.from(wanted, (path) => ensureCached(cache, path)));
  for (const request of await cache.keys()) {
    const path = new URL(request.url).pathname;
    if (path.startsWith("/assets/") && !wanted.has(path)) await cache.delete(request);
  }
}

/** The cached response for a path, fetched and stored first when missing; null if that fails. */
async function ensureCached(cache, path) {
  const cached = await cache.match(path);
  if (cached) return cached;
  try {
    const response = await fetch(path);
    if (!cacheable(response)) return null;
    await cache.put(path, response.clone());
    return response;
  } catch {
    return null;
  }
}

async function immutableAsset(request) {
  const cached = await caches.match(request, { cacheName: CACHE });
  if (cached) return cached;
  const response = await fetch(request);
  if (cacheable(response)) {
    const cache = await caches.open(CACHE);
    await cache.put(request, response.clone());
  }
  return response;
}

async function networkFirst(request) {
  try {
    return await fetch(request);
  } catch (error) {
    const cached = await caches.match(request, { cacheName: CACHE });
    if (cached) return cached;
    throw error;
  }
}

// Web Push (ADR-0017). The server sends one encrypted JSON message per device (the browser
// decrypts it): { title, body, tag, url, event, severity }. Notifications of one alert share a
// tag, so a newer one (still firing, resolved) replaces the older and alerts again (renotify).

self.addEventListener("push", (event) => {
  event.waitUntil(showPush(event.data));
});

async function showPush(data) {
  let message;
  try {
    message = data ? data.json() : {};
  } catch {
    message = { body: data ? data.text() : "" };
  }
  const title = typeof message.title === "string" ? message.title : "hetzner-monitor";
  const tag = typeof message.tag === "string" ? message.tag : undefined;
  await self.registration.showNotification(title, {
    body: typeof message.body === "string" ? message.body : "",
    tag,
    renotify: Boolean(tag),
    requireInteraction: message.severity === "critical" && message.event !== "resolved",
    icon: "/icons/icon-192.png",
    badge: "/icons/icon-192.png",
    timestamp: Date.now(),
    data: { url: safePath(message.url) },
  });
}

/** Only same-origin paths: a message must not open another site (also not via "//" or "/\\"). */
function safePath(url) {
  if (typeof url !== "string") return "/alerts";
  try {
    const resolved = new URL(url, self.location.origin);
    if (resolved.origin === self.location.origin) {
      return resolved.pathname + resolved.search + resolved.hash;
    }
  } catch {
    // Not a URL: fall through.
  }
  return "/alerts";
}

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const path = safePath(event.notification.data?.url);
  event.waitUntil(openPath(path));
});

/** Focuses an open window of the app and navigates it to `path`, or opens a new one. */
async function openPath(path) {
  const target = new URL(path, self.location.origin).href;
  // Only windows this worker controls can be navigated (an uncontrolled one, e.g. after a hard
  // reload, would reject); those are same-origin by definition.
  const windows = await self.clients.matchAll({ type: "window" });
  for (const client of windows) {
    try {
      const focused = await client.focus();
      if (focused.url !== target) await focused.navigate(target);
      return;
    } catch {
      break;
    }
  }
  await self.clients.openWindow(target);
}
