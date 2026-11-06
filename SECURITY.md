# Security policy

hetzner-monitor stores API tokens for other people's infrastructure. We take reports seriously.

## Reporting a vulnerability

Please **do not open a public issue**. Use GitHub's private vulnerability reporting:
**Security → Report a vulnerability** on this repository.

Include affected version/commit, impact, and reproduction steps. You will get an acknowledgement
within 7 days.

## Scope highlights

- Disclosure of stored Hetzner tokens or webhook URLs (DB, logs, API, errors)
- Cross-tenant data access
- Authentication/session weaknesses
- SSRF via webhook/ntfy targets

## Supported versions

Pre-release: only `main` is supported.
