# Plan: M5b — OpenAPI, self-hosting guide, release v0.1.0

- Status: Done (owner real check deferred to the web UI; `v0.1.0` tag pending)
- Milestone: M5 (second half; M5a is [its own plan](2026-11-12-m5a-auth-sessions-rotation.md))
- Owner: agent (Claude), reviewed by the maintainer
- Started: 2026-11-12
- PR: https://github.com/maxnoelp2/hetzner-monitor/pull/13

## Goal

Someone who has never seen the repository can run hetzner-monitor from the README alone, using a released multi-arch
image from GHCR, and the M6 PWA can generate its TypeScript client from an OpenAPI 3.1 document that is generated from
the code and cannot drift from the router.

## Acceptance criteria

- [x] `GET /api/v1/openapi.json` (unauthenticated) returns an OpenAPI **3.1** document generated from the code with
      `utoipa` 6 + `utoipa-axum` 0.3. It lists every route of the router (`/api/v1/*`, `/healthz`, `/readyz`), with request
      bodies, query/path parameters, typed success responses and the error responses each handler can return.
- [x] Security schemes: `api_key` (HTTP Bearer, `hm_…`), `session` (API key in cookie `hm_session`) and `csrf` (API key in
      header `X-CSRF-Token`). Read operations accept `api_key` **or** `session`; state-changing operations accept `api_key`
      **or** `session` + `csrf`; `POST /auth/login`, `/healthz`, `/readyz` and the spec itself need none. The required
      scope (`read`/`write`/`admin`) is stated per operation.
- [x] Shared `Error` schema `{error: {code, message}}`; `code` documented as the list of implemented codes.
- [x] Every operation has a unique, stable `operationId` (`listProjects`, `createChannel`, …) — needed by client generators.
- [x] `hetzner-monitor openapi` prints the same document (no database, no keys), so clients and CI can use it offline.
- [x] Tests: the document is valid JSON, `openapi` is `3.1.x`, every `$ref` resolves, operationIds are unique; every
      operation in the document is answered by the real router (not 404/405); the API router is built only through
      `utoipa-axum` (a guard test fails if a plain `.route(` appears in `crates/server/src/api/`), so a new route cannot
      be added without appearing in the document. CI additionally lints the document with Redocly CLI (pinned).
- [x] Handler responses are typed structs (`Serialize` + `ToSchema`) instead of `json!` values, so the schema is derived
      from exactly what is serialised. The JSON on the wire stays **byte-compatible** (existing integration tests unchanged).
- [x] README: a self-hosting guide that works on its own — prerequisites, getting the compose file, `.env` with generated
      master key and Postgres password, `docker compose up -d`, `bootstrap` → admin API key + initial password, logging in,
      connecting a project with a **read-only** token, first channel + test, upgrading, backups (database **and** master
      key), pointer to key rotation, reverse proxy with `HM_BASE_URL` and the `Secure` cookie, troubleshooting.
- [x] `deploy/docker-compose.yml` uses `ghcr.io/maxnoelp2/hetzner-monitor:${HM_VERSION:-0.1.0}`; the build-from-source
      variant moves to an override file `deploy/docker-compose.build.yml` for development.
- [x] Release workflow on tag `v*`: checks that the tag equals the workspace version, builds linux/amd64 and linux/arm64
      **natively** (`ubuntu-latest` + `ubuntu-24.04-arm`, no QEMU), pushes by digest, merges one multi-arch manifest to
      `ghcr.io/maxnoelp2/hetzner-monitor` with tags `0.1.0`, `0.1`, `latest`, and creates the GitHub release with the
      matching `CHANGELOG.md` section. Least-privilege `permissions` per job (`packages: write` only where pushing).
- [x] Workspace version `0.1.0`; `CHANGELOG.md` (Keep a Changelog) with the 0.1.0 entry; `hetzner-monitor --version` and the
      spec's `info.version` show it.
- [x] Docs updated: `docs/design/api.md` (spec, auth schemes), `docs/design/configuration.md` (`HM_VERSION` for compose),
      `CLAUDE.md` (commands, status), `ARCHITECTURE.md` status, roadmap, ADR-0014 (release & versioning), this plan.
- [ ] Real check by the owner: a fresh install following only the README, in a throwaway directory with a throwaway
      Compose project name and ports, against a **test** Hetzner project (never the production servers). — Deferred by owner decision (2026-11-13): tested
      together with the web UI; README steps 2–5 were walked through by the agent (progress log).

## Context

Read and verified on 2026-11-12:

- `CLAUDE.md`, `ARCHITECTURE.md`, `docs/roadmap.md` (M5), the "Split of M5" in the M5a plan, `docs/design/api.md`,
  `docs/design/security.md`, `docs/design/configuration.md`, ADR-0013.
- `crates/server/src/http.rs` (router: `/healthz`, `/readyz`, nest `/api/v1`), `crates/server/src/api/mod.rs`
  (26 operations on 17 paths), handlers in `crates/server/src/api/*.rs`: all return `Json<Value>` built with `json!`
  (25 call sites); the row types (`ProjectView`, `RuleView`, `AlertView`, …) in `crates/server/src/repo/` are plain
  `Serialize` structs with `String`/`Uuid`/`OffsetDateTime`/`Json<Value>` fields — straightforward to derive `ToSchema`.
- `crates/server/src/error.rs`: `ApiError` → `{error: {code, message}}`.
- `Dockerfile` (distroless, `healthcheck` subcommand), `deploy/docker-compose.yml` (builds locally, image `hetzner-monitor:dev`,
  binds 127.0.0.1), `.github/workflows/ci.yml` (fmt, clippy, test, docker build with `setup-buildx-action@v3`/`build-push-action@v6`).
- Workspace version is `0.0.0`; no `CHANGELOG.md` yet. The repository is **public**, so GitHub's arm64 runners
  (`ubuntu-24.04-arm`) are available at no cost.

Versions (crates.io / GitHub releases, checked 2026-11-12):

| Dependency | Version | Notes |
|---|---|---|
| `utoipa` | 6.0.0 (2026-09-22), MSRV 1.88 | features `macros`, `uuid`, `time`; `utoipa-gen` 6.0.1 |
| `utoipa-axum` | 0.3.0 (2026-09-22) | needs `axum ^0.8.4` (workspace: 0.8.9), `utoipa ^6.0.0`; pulls `pastey`, `tower-layer`, `tower-service` (latter two already in the tree via axum) |
| `docker/login-action` | v4 (4.8.0) | |
| `docker/metadata-action` | v6 (6.2.0) | |
| `docker/build-push-action` | v7 (7.4.0) | |
| `docker/setup-buildx-action` | v4 (4.4.1) | |
| `actions/upload-artifact` / `download-artifact` | v7 / v8 | digests between the per-arch jobs and the merge job |
| `@redocly/cli` | 2.77.0 | CI lint only, run with `npx` — not a project dependency |

**New crates — justification:** `utoipa` + `utoipa-axum` are the de-facto standard for OpenAPI from Axum code (already named
in `api.md` and the M5a plan). The alternative, a hand-written spec, drifts from the code; `aide` is less used and has no
3.1-first story. `utoipa-axum` registers route and documentation from the same `#[utoipa::path]` attribute, which is what
makes "cannot forget a route" enforceable. Only `hm-server` depends on them. No Swagger UI crate (it bundles assets; the
PWA and external tools can render the JSON).

## Approach

1. [x] Dependencies: `utoipa` (`macros`, `uuid`, `time`) and `utoipa-axum` in `[workspace.dependencies]`, used by `hm-server`.
       Workspace version `0.1.0`.
2. [x] Schemas: derive `ToSchema` on the repo view types and request bodies; `Json<Value>` fields (`labels`, `params`, event
       `data`) as free-form objects; closed string sets (`severity`, alert `state`, resource `kind`, channel `type`, project
       `status`, `role`, `auth`) documented as string enums via `#[schema(value_type = …)]` so the TS client gets literal
       unions. Response envelopes as typed structs (`ProjectEnvelope { project }`, `AlertList { alerts, meta }`, …); the
       `cost_estimate` that `GET /projects/{id}` injects today becomes a `#[serde(flatten)]` struct. Compare wire JSON
       before/after with the existing integration tests (they assert on the JSON).
3. [x] Paths: `#[utoipa::path]` on every handler with `operation_id`, tag, params, request body, responses (success +
       error codes), `security`; the scope in the description. Router built with `OpenApiRouter` + `routes!`; `/healthz`,
       `/readyz` included; `split_for_parts()` yields the Axum router and the `OpenApi` document.
4. [x] `openapi.rs`: top-level `#[derive(OpenApi)]` with `info` (title, version from `CARGO_PKG_VERSION`, license
       `AGPL-3.0-only`), security schemes via a `Modify`, tags; `GET /api/v1/openapi.json` serves the document (built once,
       held in an `Arc`); CLI `hetzner-monitor openapi` prints it.
5. [x] Tests (`crates/server/tests/openapi.rs`): validity checks, every documented operation reaches a handler, security
       per operation as specified above, guard against plain `.route(` in `api/`. CI job `openapi`: `cargo run -- openapi
       > openapi.json` + `npx --yes @redocly/cli@2.77.0 lint` (rules tuned in `redocly.yaml` only where justified).
6. [x] Compose: `deploy/docker-compose.yml` pulls the GHCR image (`HM_VERSION`, default `0.1.0`); new
       `deploy/docker-compose.build.yml` adds `build:` and image `hetzner-monitor:dev`. Update `CLAUDE.md` commands,
       smoke-script hints and `deploy/.env.example` (`HM_VERSION`).
7. [x] Release workflow `.github/workflows/release.yml` (tag `v*`): `verify` (tag = workspace version, CHANGELOG has the
       section) → `build` matrix amd64/arm64 on native runners, push by digest, `cache-from/to: type=gha,scope=<arch>` →
       `merge` (`docker buildx imagetools create` with `metadata-action` tags `{{version}}`, `{{major}}.{{minor}}`, `latest`;
       the `{{major}}` tag is skipped while 0.x) → `release` (`gh release create` with the CHANGELOG section). OCI labels
       from `metadata-action` link the package to the repository. Also bump the CI `docker` job to buildx v4 / build-push v7.
8. [x] `CHANGELOG.md` with 0.1.0 (M0–M5 summary, known limits: no PWA yet, no RAM/disk metrics).
9. [x] README rewrite: what it is, status, quick start / self-hosting guide (steps from the acceptance criteria), a Caddy
       reverse-proxy example, API usage with curl and the OpenAPI link, development section (build override). Remove
       "design phase". Each command tested by me locally against a throwaway Compose project (with a locally built image
       tagged like the GHCR one, since the release does not exist yet).
10. [x] ADR-0014 (Proposed): release and versioning — SemVer, `v*` tags cut by a maintainer, multi-arch image on GHCR with
       `X.Y.Z` / `X.Y` / `latest`, pre-1.0 compatibility promise (DB migrations forward-only, API under `/api/v1` may
       still change in minor releases and is listed in the CHANGELOG), compose pins an exact version.
11. [x] Docs (`api.md`, `configuration.md`, `CLAUDE.md`, `ARCHITECTURE.md`, roadmap), fmt + clippy + `cargo test --all`, PR.
12. [ ] (deferred, see progress log) Owner: real check from the README; after merge, the owner tags `v0.1.0`, the workflow publishes; the owner sets the
       GHCR package to **public** once (new packages are private by default) and pulls it anonymously.

## Out of scope

- Pushing the tag or running the release — the owner tags.
- Swagger UI / Redoc page served by the binary (the PWA or any external viewer can render the JSON).
- `PATCH /projects/{id}` and `GET /resources/{id}/metrics` (still "proposed" in `api.md`; documented once implemented).
- Helm chart, Kubernetes manifests, binary downloads, signed images (cosign) — later if asked for.
- The PWA client generation itself (M6).
- Bumping `actions/checkout` v5 → v7 in the existing CI jobs (separate chore; the new workflow uses v7).

## Risks / open questions

- **Typed responses are a broad but mechanical refactor** (≈26 handlers). Risk of changing the wire JSON (field order is
  irrelevant, but `null` vs. missing fields matters). Mitigation: no `skip_serializing_if` where the code emits `null`
  today; the existing integration tests assert on the JSON; one PR commit per area.
- **GHCR visibility:** the first push creates a private package; anonymous `docker pull` fails until the owner makes it
  public in the package settings. Documented in the release steps and the PR.
- **arm64 runner availability:** `ubuntu-24.04-arm` is for public repositories; if the repository goes private, the
  workflow needs QEMU (slow for Rust) or paid runners. Noted in ADR-0014.
- **Redocly recommended rules** may flag style issues (e.g. missing `servers`); fix in the spec rather than disabling rules,
  and justify every disabled rule in `redocly.yaml`.
- **`deploy/.env.example` is blocked by this agent's permission rules** (pattern for `.env` files). I need the owner's
  permission to read and edit `deploy/.env.example` (never `deploy/.env`); otherwise the owner adds the `HM_VERSION` line.
- The README's "connect a project" step needs a read-only token from the Hetzner console; the real check must use a test
  project, never the production one.

## Progress log

- 2026-11-12 — Plan drafted after reading the docs and code listed above; versions re-verified.
- 2026-11-12 — Approved by the owner. Implemented:
  - utoipa's `Modify` runs when `ApiDoc::openapi()` is built, i.e. **before** `utoipa-axum` adds the paths, so the
    per-operation security and shared responses are applied by `openapi::finish()` after `split_for_parts()`.
  - The ref check found that schemas referenced only from query parameters (`AlertFilter`, `ResourceKind`) are not
    collected automatically; they are listed in `components(schemas(...))`.
  - Redocly found responses without `description` (required in 3.1); `finish()` fills the status text. An untagged
    `oneOf` for channel configs needed `additionalProperties: false` to be mutually exclusive.
  - Response schemas mark all fields required (they are always serialised, `None` as `null`) except an explicit list
    of fields skipped when empty; request schemas are listed explicitly.
  - Wire JSON unchanged: the existing integration tests pass without modification.
  - `clippy.toml` allows `OpenAPI`, `HttpOnly`, `SameSite` in doc comments (they become operation descriptions).
  - Dev commands now need `-f deploy/docker-compose.build.yml`; `deploy/docker-compose.yml` alone pulls the release.
  - `deploy/.env.example` could not be read or edited by the agent (permission rule for `.env*`); `HM_VERSION` is
    documented in configuration.md and the README instead.
- 2026-11-13 — CI green on the PR (5 checks). Local image build of this branch works (`openapi` prints 3.1.0,
  `info.version` 0.1.0). The agent walked through README steps 2–5 in a throwaway directory with the image tagged
  `ghcr.io/maxnoelp2/hetzner-monitor:0.1.0` locally: start, `/readyz`, `bootstrap` (second run refused), `/me`, login with
  cookie, POST without CSRF → 403, password change → 204. Steps 6–7 (test token, channel test) not done: the owner
  deferred the real check until the web UI is complete. Finding: the Compose file names the project `hetzner-monitor`,
  so a second install on the same host shares containers with the first unless started with `-p <name>` — the first
  test run replaced the owner's dev containers (Postgres kept its volume and refused the new password, so no data
  changed; restored at once). README now says so.
