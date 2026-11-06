# hetzner-monitor

Monitoring & alerting for **Hetzner Cloud** — paste a read-only API token, get meaningful alerts.
No agent on your servers, no Prometheus stack.

> **Status: design phase.** No usable code yet. See the [roadmap](docs/roadmap.md).

## Planned features (MVP)

- Connect Hetzner Cloud projects with a read-only token (stored encrypted)
- Auto-discovered inventory: servers, load balancers, volumes, certificates, IPs
- Alerts for availability, CPU/network/disk I/O, **traffic quota with forecast**, certificate expiry, deprecations, hygiene and cost
- Rules with thresholds, durations, label selectors; silences and maintenance windows
- Notifications via email, Slack, Discord, webhook, ntfy, Web Push
- Self-hosted via Docker Compose (Rust + PostgreSQL); hosted version later

Limits: the Hetzner API provides no RAM or disk-fill metrics — those need an agent (later phase).

## Documentation

- [Architecture](ARCHITECTURE.md)
- [Product scope](docs/product.md) · [Roadmap](docs/roadmap.md) · [Decisions (ADRs)](docs/adr/README.md)
- [All docs](docs/README.md)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). AI agents: start with [CLAUDE.md](CLAUDE.md).

## License

[AGPL-3.0](LICENSE)
