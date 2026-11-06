# syntax=docker/dockerfile:1

# ---- build ----
FROM rust:1.99.1-slim-trixie AS builder
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p hm-server \
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
