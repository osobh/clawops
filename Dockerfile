# ─── Stage 1: Builder ────────────────────────────────────────────────────────
FROM rust:1.93-slim-bookworm AS builder

# Install system dependencies for building
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    libssh2-1-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Cache dependencies by copying manifests first
COPY Cargo.toml Cargo.lock ./
COPY gf-node-proto/Cargo.toml gf-node-proto/
COPY gf-provision/Cargo.toml gf-provision/
COPY gf-health/Cargo.toml gf-health/
COPY gf-failover/Cargo.toml gf-failover/
COPY gf-metrics/Cargo.toml gf-metrics/
COPY gf-audit/Cargo.toml gf-audit/
COPY gf-clawnode/Cargo.toml gf-clawnode/

# Create stub lib/main files for dependency caching
RUN for crate in gf-node-proto gf-provision gf-health gf-failover gf-metrics gf-audit; do \
    mkdir -p $crate/src && echo "pub fn _stub() {}" > $crate/src/lib.rs; \
    done && \
    mkdir -p gf-clawnode/src && echo "fn main() {}" > gf-clawnode/src/main.rs

RUN cargo build --release --bin gf-clawnode 2>/dev/null || true

# Now copy real source and build
COPY gf-node-proto/src gf-node-proto/src
COPY gf-provision/src gf-provision/src
COPY gf-health/src gf-health/src
COPY gf-failover/src gf-failover/src
COPY gf-metrics/src gf-metrics/src
COPY gf-audit/src gf-audit/src
COPY gf-clawnode/src gf-clawnode/src

# Touch source files to invalidate the stub cache
RUN find . -name "*.rs" -not -path "*/target/*" -exec touch {} +

RUN cargo build --release --bin gf-clawnode

# Strip the binary to minimize size
RUN strip target/release/gf-clawnode

# ─── Stage 2: Runtime ─────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

# Install minimal runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    libssh2-1 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user for running the agent
RUN groupadd -r clawnode && useradd -r -g clawnode -s /bin/false clawnode

# Create required directories
RUN mkdir -p /etc/gf-clawnode /var/log/gf-clawnode && \
    chown -R clawnode:clawnode /var/log/gf-clawnode

# Copy binary from builder
COPY --from=builder /build/target/release/gf-clawnode /usr/local/bin/gf-clawnode
RUN chmod +x /usr/local/bin/gf-clawnode

# Health check: verify the binary can start and respond
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -sf http://localhost:8090/health || exit 1

# Default config path (override with GF_CONFIG_FILE env var)
ENV GF_CONFIG_FILE=/etc/gf-clawnode/config.toml

USER clawnode

ENTRYPOINT ["/usr/local/bin/gf-clawnode"]
CMD ["--config", "/etc/gf-clawnode/config.toml"]
