# ADR-0016: Frontend tooling for the PWA

- Status: Proposed
- Date: 2026-11-13
- Deciders: Max Nölp (owner), Claude (agent, draft)

## Context

The PWA ([ADR-0009](0009-frontend-react-tauri.md)) is a CRUD-and-status app on top of a typed REST API whose OpenAPI
3.1 document is generated from the server code (M5b). It will later be wrapped by Tauri 2. The project values few
dependencies, a small attack surface and contributors who can work on it without frontend specialists. Versions checked
on npm on 2026-11-13.

## Decision

- **Language and framework:** TypeScript 5.9 (strict), React 19, Vite 8 (dev server proxies `/api` to the Rust server).
- **Routing:** React Router 8 in declarative (library) mode, no framework/SSR mode.
- **Server state:** TanStack Query 5 (caching, retries, invalidation after mutations). No global client-state library.
- **API client:** types generated with `openapi-typescript` from `hetzner-monitor openapi` into
  `web/src/api/schema.d.ts` (committed; CI checks it is current) and called with `openapi-fetch` (small, typed
  `fetch` wrapper). One middleware adds `X-CSRF-Token` to non-GET requests and turns 401 into a return to login.
- **Look:** the owner's design system (dark-first tokens, Geist, glass cards, pill buttons) re-implemented as plain
  React components and CSS custom properties in `web/src/ui/` — no component library, no CSS framework. Icons from
  `lucide-react` (tree-shaken); fonts from `@fontsource-variable/geist` and `…/geist-mono` (OFL, bundled).
- **PWA:** a hand-written `public/sw.js` and `manifest.webmanifest`; no Workbox.
- **Quality:** ESLint 10 with `typescript-eslint` and `eslint-plugin-react-hooks`, Prettier, Vitest 5 + Testing
  Library + jsdom. npm with a committed `package-lock.json`, installed with `npm ci`.
- UI copy is English for now (same language as the code and docs).

## Consequences

- **Pro:** the client cannot drift from the API; the runtime bundle stays small (React, Router, Query, openapi-fetch,
  the used Lucide icons); the stack is mainstream, so Tauri and contributors meet no surprises.
- **Con:** components are maintained by us instead of a library (tables, dialogs, focus handling need care and tests).
- **Con:** TypeScript 6/7 are not yet supported by `openapi-typescript` (peer `^5`) and `typescript-eslint` (`<6.1`); we stay on 5.9 until they are.
- Rules out: Next.js/SSR, Redux/Zustand, Tailwind or a component kit, Workbox, Google Fonts.
