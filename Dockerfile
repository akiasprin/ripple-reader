# syntax=docker/dockerfile:1

# =============================================================================
# Stage 1: Builder
# =============================================================================
FROM rust:1.85-slim-bookworm AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
        libpq-dev \
    && rm -rf /var/lib/apt/lists/*

# Cache dependencies: copy manifests first
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs
RUN cargo build --release 2>/dev/null || true

# Copy source and build
COPY src ./src
COPY prompts ./prompts
COPY static ./static
COPY migrations ./migrations
RUN touch src/main.rs
RUN cargo build --release

# =============================================================================
# Stage 2: Runtime
# =============================================================================
FROM debian:bookworm-slim AS runtime

WORKDIR /app

# Install runtime dependencies
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        libssl3 \
        libpq5 \
    && rm -rf /var/lib/apt/lists/*

# Copy binaries from builder
COPY --from=builder /app/target/release/ripple-reader /usr/local/bin/ripple-reader
COPY --from=builder /app/target/release/redo_mineru /usr/local/bin/redo_mineru
COPY --from=builder /app/target/release/mdfmt /usr/local/bin/mdfmt
COPY --from=builder /app/target/release/warmup_insight_cache /usr/local/bin/warmup_insight_cache

# Copy runtime assets
COPY --from=builder /app/prompts ./prompts
COPY --from=builder /app/static ./static
COPY --from=builder /app/migrations ./migrations

# Create non-root user
RUN useradd -m -u 1000 appuser && chown -R appuser:appuser /app
USER appuser

EXPOSE 8181

ENTRYPOINT ["ripple-reader"]
CMD ["web"]
