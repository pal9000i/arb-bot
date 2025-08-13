# Dockerfile for Arrakis Arbitrage Service
# Multi-stage build for production optimization

# ---------- Build stage ----------
FROM rust:1.89.0 AS builder

# Install system dependencies for building
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# Create app user
RUN adduser --disabled-password --gecos '' --uid 1000 appuser

WORKDIR /app

# Copy dependency files first for better caching
COPY Cargo.toml Cargo.lock ./

# Build deps layer (dummy main to cache deps)
RUN mkdir src && echo "fn main() {}" > src/main.rs \
 && cargo build --release \
 && rm -rf src

# Copy source
COPY src/ ./src/
COPY abis/ ./abis/

# Build the actual application
RUN cargo build --release

# ---------- Runtime stage ----------
FROM debian:bookworm-slim

# Install runtime dependencies (need curl for HEALTHCHECK, and OpenSSL for TLS)
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    curl \
 && rm -rf /var/lib/apt/lists/*

# Create app user (same UID)
RUN adduser --disabled-password --gecos '' --uid 1000 appuser

WORKDIR /app

# Copy binary
COPY --from=builder /app/target/release/arrakis_arbitrage ./arrakis_arbitrage
# Copy runtime ABIs
COPY --from=builder /app/abis/ ./abis/

# Set permissions
RUN chown -R appuser:appuser /app

USER appuser

EXPOSE 8000

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=60s --retries=3 \
  CMD curl -fsS http://localhost:8000/health || exit 1

# Environment defaults
ENV RUST_LOG=info
ENV ROCKET_ADDRESS=0.0.0.0
ENV ROCKET_PORT=8000

CMD ["./arrakis_arbitrage"]
  