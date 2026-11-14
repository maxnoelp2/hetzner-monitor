# Design: Web UI (PWA)

Status: **implemented** (M6a, Web Push M6b, [plan](../plans/2026-11-13-m6-pwa-web-push.md)) ·
Decisions: [ADR-0015](../adr/0015-web-ui-embedded-in-binary.md) (embedded in the binary),
[ADR-0016](../adr/0016-frontend-tooling.md) (tooling), [ADR-0017](../adr/0017-web-push.md) (Web Push) · Code: `web/`

The web UI is a React single-page app that the `hetzner-monitor` binary serves on the same origin as `/api/v1`. It uses
only the public HTTP API ([api](api.md)); there are no UI-only endpoints.

## Structure of `web/`

```
web/
  index.html            Vite entry (no inline script or style: the CSP forbids them)
  public/               copied as-is into dist/: manifest.webmanifest, sw.js, icons/
  src/
    main.tsx            mounts <App>, imports fonts and CSS, registers the service worker (production builds only)
    App.tsx             providers (TanStack Query, toasts, session, router) and the route table
    Layout.tsx          app shell: header with navigation, org name, logout
    api/client.ts       openapi-fetch client, CSRF and 401 middleware, unwrap(), ApiError, errorText()
    api/schema.d.ts     generated from the OpenAPI document (committed, never edited by hand)
    api/session.tsx     SessionProvider, useSession(), useProfile()
    pages/              one module per area (Alerts, Projects, Rules, Channels, Silences, Settings, Login, NotFound),
                        tests next to them (<Page>.test.tsx)
    ui/                 design tokens (tokens.css), component styles (ui.css), components.tsx, toast.tsx
    format.ts           labels, badge tones, date/duration/money formatting
    push.ts             Web Push in the browser: support check, permission, subscribe, "this device" id
    test/               Vitest setup and helpers (mockFetch, renderApp)
  dist/                 build output, embedded into the binary (not committed)
```

Stack ([ADR-0016](../adr/0016-frontend-tooling.md)): TypeScript 5.9 (strict), React 19, Vite 8, React Router 8
(declarative mode), TanStack Query 5, `openapi-fetch` with types from `openapi-typescript`, `lucide-react`, Geist and
Geist Mono from `@fontsource-variable` (bundled). npm with a committed `package-lock.json`.

## Routes

| Path | Screen |
|---|---|
| `/` | redirects to `/alerts` |
| `/alerts`, `/alerts/:id` | alert list (state and project filters), detail with events, deliveries and ack |
| `/projects`, `/projects/:id` | project list and connect; detail with schedule, budget, cost estimate, inventory by kind, sync |
| `/rules` | rules: enable/disable, edit, create and delete custom rules |
| `/channels` | channels: create/edit per type, send a test, delete |
| `/silences` | active silences, create, end |
| `/settings` | account (change password), push notifications (this device, your devices, test, remove) and API keys (create, shown once; revoke) |

Any other path shows a "not found" page inside the app. Without a session every path renders the login form; after
login the requested path is shown. Notification links point to `<HM_BASE_URL>/alerts/{id}`
([configuration](configuration.md)).

## Auth flow

- **Login:** e-mail and password (`POST /api/v1/auth/login`); the first login uses the e-mail and the initial password
  that `hetzner-monitor bootstrap` prints (`users reset-password` prints a new one). There is no API-key login in the
  UI. The server sets the `HttpOnly` cookie `hm_session` ([security](security.md#authentication)); the response's
  `csrf_token` is kept **in memory only** (a module variable in `api/client.ts`, never `localStorage`,
  `sessionStorage` or a cookie).
- **Reload:** on start the app calls `GET /api/v1/me`; 200 restores the profile and re-reads the CSRF token, 401 shows
  the login form.
- **CSRF:** a middleware adds `X-CSRF-Token` to every request that is not `GET`/`HEAD`/`OPTIONS`.
- **401:** any 401 other than a failed login clears the CSRF token and the query cache and returns to the login form.
- **Logout:** `POST /api/v1/auth/logout`, then the same local clean-up.
- **Roles:** the profile's `role` (`owner`/`admin` → `admin` scope, `member` → `write`) decides which actions are shown;
  the server enforces the scope regardless.

## Data fetching

- Server state lives in TanStack Query; there is no other global state. Query keys: `["alerts", …]`, `["alert", id]`,
  `["projects"]`, `["project", id]`, `["resources", id, kind]`, `["rules", …]`, `["rule", id]`, `["channels"]`,
  `["silences", …]`, `["api-keys"]`, `["push-config"]`, `["push-subscriptions"]`. Mutations invalidate the matching keys.
- Queries are fresh for 15 s and refetch on window focus. Client errors (4xx) are not retried; others up to twice.
- `unwrap(call)` returns the data or throws `ApiError(status, code, message)` with the server's error body
  ([api](api.md#conventions)); screens show `errorText(e)`.

## Generated API client

`web/src/api/schema.d.ts` is generated from the server's OpenAPI document and committed. After an API change:

```bash
cargo run -q -p hm-server -- openapi > openapi.json && (cd web && npm run gen:api)
```

CI regenerates it and fails if the committed file differs. Paths in calls are the full API paths
(`api.GET("/api/v1/alerts", …)`), so a renamed or removed endpoint is a type error.

## Design

The owner's design system (dark-first tokens, Geist, glass cards, pill buttons, Lucide icons at 16 px / 1.5 stroke) is
re-implemented in `web/src/ui/` without its logo, plus the parts a monitoring UI needs that it lacks: tables, severity
and state badges, empty, loading and error states. Rules for screens:

- Sentence case, short verb-first buttons ("Connect project", "Send test"), plain copy, no emoji. UI copy is English.
- One primary (pink) button per view; other actions outline or ghost; destructive actions use the danger style and
  ask for confirmation in a dialog.
- Usable at 375 px width: tables scroll horizontally inside `.table-wrap`, secondary columns are hidden on small
  screens (`hide-mobile`).
- Every data view has a loading, an empty and an error state.
- Secrets are never shown, except a newly created API key once. Channel configs are write-only (only the `hint` is
  displayed); a Hetzner token is entered once on connect, with the hint that a **Read** token is enough.

## Serving and headers

With the cargo feature `web-ui`, `hm-server` embeds `web/dist` with `rust-embed`. The files are served by the router's
**fallback**, not as routes, so the OpenAPI document does not change ([ADR-0015](../adr/0015-web-ui-embedded-in-binary.md)):

1. `/api` and anything under `/api/` → the JSON `404 not_found` of the API, for every method (never `index.html`).
2. A `GET`/`HEAD` path that names an embedded file → that file with its MIME type and a strong `ETag` (SHA-256;
   `If-None-Match` → `304`). Paths with empty, `.` or `..` segments or backslashes never name a file.
3. A `GET`/`HEAD` path under `/assets/` that names no embedded file → the JSON 404 (a stale hashed URL must not get
   `index.html`, which would be cached as an immutable script or font).
4. Any other `GET`/`HEAD` path → `index.html` (client-side routing).
5. Other methods outside `/api` → the JSON 404 as well.

Without the feature (plain `cargo build`, `cargo test`), every non-`/api` `GET` path serves a short placeholder page
that explains how to build the UI; the same happens with the feature when `web/dist` did not exist at build time
(`allow_missing`, so CI can run clippy and tests with all features without building the UI). The Docker image (a Node
stage builds `web/dist`) and the release builds enable the feature. Code: `crates/server/src/web.rs`.

Headers on every response of the fallback (UI files, `index.html`, the placeholder and the JSON 404s):

| Header | Value |
|---|---|
| `Content-Security-Policy` | `default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self'; font-src 'self'; connect-src 'self'; manifest-src 'self'; worker-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'` |
| `X-Content-Type-Options` | `nosniff` |
| `Referrer-Policy` | `same-origin` |
| `Cache-Control` | `/assets/*` (content-hashed by Vite): `public, max-age=31536000, immutable`; everything else (`index.html`, `sw.js`, `manifest.webmanifest`, icons): `no-cache` |

The UI makes no third-party requests: fonts and icons are bundled, and Vite inlines nothing (`assetsInlineLimit: 0`),
so the CSP needs no `data:` or inline exceptions. Security notes: [security](security.md#web-ui).

## PWA and service worker

- `manifest.webmanifest` (name, icons, `display: standalone`, theme colours) makes the app installable ("Add to Home
  Screen" / "Install app"). Browsers allow this and service workers only in a secure context: HTTPS, or
  `http://localhost` / `127.0.0.1`.
- `sw.js` is hand-written plain JavaScript (no Workbox, no build step), registered from `/sw.js` in production builds,
  so its scope is the whole origin. It caches the **app shell only**: `index.html` (stored as `/`), the manifest, the
  icons and the `/assets/` files the current `index.html` references (scripts, stylesheets and their fonts). The UI
  therefore starts without a network and shows its own error states when the server is unreachable.
- Strategies: navigations are network-first with the cached shell as offline fallback; `/assets/*` (content-hashed)
  cache-first; other same-origin `GET`s network-first. Files of older builds are dropped from the cache; changing the
  caching scheme means bumping the cache name (`hm-shell-vN`).
- **API responses are never cached**: the worker does not touch requests under `/api`, so sessions, CSRF and alert
  state are always live and no alert, project or channel data is stored by the browser.
- `start_url` is `/alerts`; installed apps open there.
- **Push** (M6b, [ADR-0017](../adr/0017-web-push.md)): the `push` handler shows the decrypted message
  `{title, body, tag, url, event, severity}` as a notification (app icon, one `tag` per alert with `renotify`, so a
  resolve replaces the firing notification; critical alerts stay until dismissed). `notificationclick` focuses an open
  window of the app and navigates it to the message's path (`/alerts/{id}`, `/settings` for tests), or opens one. Only
  same-origin paths are used.

## Web Push in the UI

Settings → **Push notifications** (code: `pages/Settings.tsx`, `push.ts`):

- `GET /push/config` says whether the server has Web Push (`HM_VAPID_SUBJECT` or an `https://` `HM_BASE_URL`) and
  gives the VAPID public key. Without it the card explains the setting and shows nothing else.
- **This device:** "Turn on push for this device" asks for the notification permission (from the click), subscribes the
  browser with the VAPID key (`PushManager.subscribe`, `userVisibleOnly`; an old subscription for another key is
  replaced) and posts it with a name like `iPhone · Safari`. The returned id is kept in `localStorage`
  (`hm-push-subscription-id`) only to mark "This device"; nothing else about the subscription is stored by the UI.
  "Send test push" and "Turn off" (server delete + browser unsubscribe) follow. **Logout** does the same for this
  device first (best effort), so a logged-out browser gets no more alerts.
- When push is impossible the card says why instead: no secure context ("Push needs HTTPS", see the README), iPhone or
  iPad outside the home screen ("Add to Home Screen", iOS 16.4+), an unsupported browser, or blocked notifications.
- **Your devices:** the caller's subscriptions with push service host, "Send test" and "Remove" (with confirmation). A
  test that reports the device gone removes it from the list.
- If the org has no `web_push` channel, the card links to the Channels page: alerts reach devices only through such a
  channel. Channels → Add channel → type **Web Push** has no settings (every member's devices); its test lists the
  result per device. Alert details show the device next to the channel for each push delivery.

## Development

```bash
cd web && npm ci                  # install dependencies; the commands below run in web/
npm run typecheck                 # tsc --noEmit
npm run lint                      # ESLint + Prettier check
npm test                          # Vitest
npm run build                     # type check + production build into web/dist
npm run dev                       # Vite on http://localhost:5173, proxies /api to http://127.0.0.1:8080 (or HM_DEV_API); keeps running
```

The dev server needs a running API (`hetzner-monitor serve` or the Compose stack). Log in with a user's e-mail and
password; the session cookie works because the proxy keeps the browser on one origin. The cookie is `Secure` unless
`HM_BASE_URL` is `http://…`; if the browser drops it on `http://localhost:5173`, run the API with
`HM_BASE_URL=http://localhost:5173`.

Building the binary with the UI: `(cd web && npm run build) && cargo build --features web-ui`. A UI change is only
visible in the binary after `npm run build`; in debug builds `rust-embed` reads `web/dist` from disk, release builds
include it.

## Testing

- **Vitest + Testing Library + jsdom**, tests next to the page (`web/src/pages/<Page>.test.tsx`). `test/utils.tsx`
  provides `mockFetch(handler)` (stubs `fetch`, records `{method, path, body, headers}`; unhandled requests answer a
  JSON 404) and `renderApp(<AppRoutes />, "/path")` (all providers and a `MemoryRouter`).
- Covered at least: the API wrapper (CSRF header on non-GET, 401 returns to login) and the login and alert-ack flows.
  Push (`pages/SettingsPush.test.tsx`) mocks the browser side (`push.ts`) and covers turning push on and off, blocked
  permission, the unsupported cases, tests and removing a device.
- Rust integration tests cover the serving rules: fallback to `index.html`, `/api` exclusion, security and cache
  headers, placeholder without the feature.
