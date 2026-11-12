# ADR-0014: Releases — SemVer tags, multi-arch image on GHCR, pinned Compose version

- Status: Proposed
- Date: 2026-11-12
- Deciders: maintainer (pending), agent (draft)

## Context

M5 ends with release v0.1.0 and the acceptance "self-hostable from the README alone" ([roadmap](../roadmap.md)). Until
now the Compose file built the image from source, which needs the repository and a Rust build (minutes, gigabytes) on
the target host. Self-hosters run on both x86 and Arm servers (Hetzner Cloud offers
both). Facts checked 2026-11-12:

- The repository is public, so GitHub's Arm runners (`ubuntu-24.04-arm`) are available at no cost. Building Rust under
  QEMU emulation on an x86 runner is many times slower.
- GHCR (`ghcr.io`) is free for public images and needs no extra secret: the workflow's `GITHUB_TOKEN` with
  `packages: write` can push. A newly created package is private until its owner makes it public once.
- `docker/metadata-action` v6, `build-push-action` v7, `setup-buildx-action` v4 and `login-action` v4 are the current
  majors; the documented pattern for native multi-platform builds pushes each platform by digest and merges them with
  `docker buildx imagetools create`.

## Decision

- **Versions follow SemVer**, one version for the whole workspace (`[workspace.package] version`). A release is a
  maintainer-pushed annotated tag `vX.Y.Z` that must equal the workspace version and have a `## [X.Y.Z]` section in
  `CHANGELOG.md` (Keep a Changelog). Agents prepare releases but never push tags.
- **Before 1.0**, a minor release (`0.Y`) may change the API under `/api/v1` or the configuration; every such change is
  listed under *Changed* in the CHANGELOG with what to do. Patch releases (`0.Y.Z`) are compatible. Database migrations
  are forward-only and run on startup; downgrades are not supported (restore the backup instead).
- **Distribution:** `.github/workflows/release.yml` builds `linux/amd64` and `linux/arm64` natively (one runner per
  platform, no QEMU), pushes by digest and publishes one manifest to `ghcr.io/maxnoelp2/hetzner-monitor` with the tags
  `X.Y.Z`, `X.Y` and `latest` (the bare `X` tag only from 1.0 on), then creates the GitHub release from the CHANGELOG
  section. Jobs get least-privilege permissions; only the build and merge jobs can write packages.
- **`deploy/docker-compose.yml` pins an exact version** (`${HM_VERSION:-X.Y.Z}`, the default bumped with each
  release). Upgrading means changing `HM_VERSION`, not following `latest`. Building from source is a development override
  (`deploy/docker-compose.build.yml`).

## Consequences

- **Pro:** a self-hoster needs only Docker, the Compose file and a `.env`; the same image runs on x86 and Arm servers;
  upgrades are explicit and reversible up to the database migration (backup first).
- **Pro:** the release is reproducible from a tag, and the CHANGELOG is the single source for release notes.
- **Con:** every release needs a version bump in `Cargo.toml`, the Compose default and the CHANGELOG in the release PR.
- **Con:** if the repository becomes private, the Arm runner is no longer free: the workflow then needs paid Arm
  runners or QEMU.
- Rules out: mutable `latest` in the Compose file; unversioned "nightly" images on GHCR (can be added later as a
  separate tag namespace); signed images and SBOM attestations are not part of 0.1.0 (possible follow-up with cosign).
