# Multi-stage build for the Arvolo relay (zero-knowledge mailbox).
# Build context is the workspace root: `docker build -f relay/Dockerfile .`

FROM rust:slim-trixie AS builder
WORKDIR /app

# C toolchain needed by some crypto deps (aws-lc-sys / ring) and rusqlite.
RUN apt-get update \
 && apt-get install -y --no-install-recommends build-essential cmake pkg-config perl \
 && rm -rf /var/lib/apt/lists/*

# 1) Cache dependencies: copy manifests + lockfile, build with dummy sources.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY core/Cargo.toml core/Cargo.toml
COPY cli/Cargo.toml cli/Cargo.toml
COPY relay/Cargo.toml relay/Cargo.toml
RUN mkdir -p core/src cli/src relay/src \
 && echo "" > core/src/lib.rs \
 && echo "fn main() {}" > cli/src/main.rs \
 && echo "fn main() {}" > relay/src/main.rs \
 && cargo build --release --locked -p arvolo-relay \
 && rm -rf core/src cli/src relay/src

# 2) Build the real binary (deps are cached above).
COPY . .
RUN cargo build --release --locked -p arvolo-relay

FROM debian:trixie-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/* \
 && useradd -r -u 10001 arvolo \
 && mkdir -p /data && chown arvolo:arvolo /data
COPY --from=builder /app/target/release/arvolo-relay /usr/local/bin/arvolo-relay

USER arvolo
ENV ARVOLO_RELAY_ADDR=0.0.0.0:8787 \
    ARVOLO_RELAY_DB=/data/relay.db \
    ARVOLO_RELAY_BLOBS=/data/blobs
EXPOSE 8787
VOLUME ["/data"]
CMD ["arvolo-relay"]
