# Multi-stage build for atem-proxy (linux/amd64, linux/arm64)
FROM rust:bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release -p atem-proxy \
    && strip target/release/atem-proxy

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/atem-proxy /usr/local/bin/atem-proxy
COPY deploy/atem-proxy.toml.example /etc/atem-proxy/atem-proxy.toml
ENV ATEM_PROXY_CONFIG=/etc/atem-proxy/atem-proxy.toml
EXPOSE 9910/udp
ENTRYPOINT ["/usr/local/bin/atem-proxy"]
CMD ["--config", "/etc/atem-proxy/atem-proxy.toml"]
