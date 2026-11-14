# syntax=docker/dockerfile:1

# ---- web UI (ADR-0015, ADR-0016) ----
# Builds web/dist. The typed API client (web/src/api/schema.d.ts) is committed, so no openapi.json is needed here.
FROM node:24.15.0-trixie-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm \
    npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run build

# ---- build ----
FROM rust:1.99.1-slim-trixie AS builder
WORKDIR /src
COPY . .
# The server embeds the built UI with the `web-ui` feature.
COPY --from=web /web/dist /src/web/dist
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p hm-server --features web-ui \
 && cp target/release/hetzner-monitor /hetzner-monitor

# ---- runtime ----
FROM gcr.io/distroless/cc-debian13:nonroot
COPY --from=builder /hetzner-monitor /usr/local/bin/hetzner-monitor
ENV HM_BIND=0.0.0.0:8080
EXPOSE 8080
USER nonroot
HEALTHCHECK --interval=10s --timeout=5s --start-period=10s --retries=3 \
  CMD ["/usr/local/bin/hetzner-monitor", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/hetzner-monitor"]
CMD ["serve"]
