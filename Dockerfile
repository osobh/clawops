# ─── Stage 1: Builder ────────────────────────────────────────────────────────
FROM rust:1.93-slim-bookworm AS builder

# Install system dependencies for building
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Cache workspace dependencies by copying manifests first
COPY Cargo.toml Cargo.lock ./
COPY crates/claw-proto/Cargo.toml      crates/claw-proto/
COPY crates/claw-persist/Cargo.toml   crates/claw-persist/
COPY crates/claw-identity/Cargo.toml  crates/claw-identity/
COPY crates/claw-secrets/Cargo.toml   crates/claw-secrets/
COPY crates/claw-auth/Cargo.toml      crates/claw-auth/
COPY crates/claw-config/Cargo.toml    crates/claw-config/
COPY crates/claw-metrics/Cargo.toml   crates/claw-metrics/
COPY crates/claw-health/Cargo.toml    crates/claw-health/
COPY crates/claw-provision/Cargo.toml crates/claw-provision/
COPY crates/claw-audit/Cargo.toml     crates/claw-audit/
COPY crates/clawnode/Cargo.toml       crates/clawnode/
COPY crates/clawops-tests/Cargo.toml  crates/clawops-tests/

# Create stub lib/main files for dependency caching
RUN for crate in claw-proto claw-persist claw-identity claw-secrets claw-auth \
        claw-config claw-metrics claw-health claw-provision claw-audit clawops-tests; do \
        mkdir -p crates/$crate/src && echo "pub fn _stub() {}" > crates/$crate/src/lib.rs; \
    done && \
    mkdir -p crates/clawnode/src && echo "fn main() {}" > crates/clawnode/src/main.rs && \
    mkdir -p crates/clawops-tests/tests

RUN cargo build --release --bin clawnode 2>/dev/null || true

# Copy real source files
COPY crates/claw-proto/src      crates/claw-proto/src
COPY crates/claw-persist/src    crates/claw-persist/src
COPY crates/claw-identity/src   crates/claw-identity/src
COPY crates/claw-secrets/src    crates/claw-secrets/src
COPY crates/claw-auth/src       crates/claw-auth/src
COPY crates/claw-config/src     crates/claw-config/src
COPY crates/claw-metrics/src    crates/claw-metrics/src
COPY crates/claw-health/src     crates/claw-health/src
COPY crates/claw-provision/src  crates/claw-provision/src
COPY crates/claw-audit/src      crates/claw-audit/src
COPY crates/clawnode/src        crates/clawnode/src

# Touch sources to invalidate stub cache
RUN find . -name "*.rs" -not -path "*/target/*" -exec touch {} +

RUN cargo build --release --bin clawnode

# Strip the binary to minimize size
RUN strip target/release/clawnode

# ─── Stage 2: Runtime ─────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

# Install minimal runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user for running the agent
RUN groupadd -r clawnode && useradd -r -g clawnode -s /bin/false clawnode

# Create required directories
RUN mkdir -p /etc/clawnode /var/lib/clawnode/state /var/log/clawnode && \
    chown -R clawnode:clawnode /etc/clawnode /var/lib/clawnode /var/log/clawnode

# Copy binary from builder
COPY --from=builder /build/target/release/clawnode /usr/local/bin/clawnode
RUN chmod +x /usr/local/bin/clawnode

# Health check: verify the agent is alive via its health endpoint
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD curl -sf http://localhost:8090/health || exit 1

# Environment variables (override in docker-compose or env file)
ENV CLAWNODE_INSTANCE_ID="" \
    CLAWNODE_ACCOUNT_ID="" \
    CLAWNODE_GATEWAY_URL="" \
    CLAWNODE_AUTH_TOKEN="" \
    CLAWNODE_PROVIDER="hetzner" \
    CLAWNODE_REGION="eu-hetzner-nbg1" \
    CLAWNODE_TIER="standard" \
    CLAWNODE_ROLE="primary" \
    CLAWNODE_STATE_DIR="/var/lib/clawnode/state" \
    RUST_LOG="clawnode=info,claw_health=info,claw_metrics=info"

USER clawnode

ENTRYPOINT ["/usr/local/bin/clawnode"]
