# Multi-stage build for the Arvolo relay (zero-knowledge mailbox).
# Build context is the workspace root: `docker build -f relay/Dockerfile .`

FROM rust:slim-trixie AS builder
WORKDIR /app

# How many crates cargo may compile at once. Empty = cargo's default (one job per
# core), which is what CI wants. Set it on a small host: this dependency graph has
# several crates whose rustc peaks around half a gigabyte, so on a 2-core/4 GB VPS
# the default fans out into an out-of-memory kill — `signal: 9, SIGKILL` on a
# random crate, with no obvious cause in the build log. Adding swap does not help
# enough; capping the fan-out does.
#   docker compose build --build-arg CARGO_JOBS=1
ARG CARGO_JOBS=""

# C toolchain needed by some crypto deps (aws-lc-sys / ring) and rusqlite.
RUN apt-get update \
 && apt-get install -y --no-install-recommends build-essential cmake pkg-config perl \
 && rm -rf /var/lib/apt/lists/*

# 1) Cache dependencies: copy manifests + lockfile, build with dummy sources.
#
# EVERY workspace member's manifest has to be here, even the ones the relay never
# links against: cargo loads the whole workspace before it can build one package
# of it, so a member whose `Cargo.toml` is missing fails the build outright. This
# step silently rotted when `arvolo-ipc` and the Tauri GUI joined the workspace
# and nobody added them — the image kept working only because it was never
# rebuilt. `gui/src-tauri` also declares a `build.rs`, so the stub needs one.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY core/Cargo.toml core/Cargo.toml
COPY cli/Cargo.toml cli/Cargo.toml
COPY relay/Cargo.toml relay/Cargo.toml
COPY arvolo-ipc/Cargo.toml arvolo-ipc/Cargo.toml
COPY gui/src-tauri/Cargo.toml gui/src-tauri/Cargo.toml
RUN mkdir -p core/src cli/src relay/src arvolo-ipc/src gui/src-tauri/src \
 && echo "" > core/src/lib.rs \
 && echo "" > arvolo-ipc/src/lib.rs \
 && echo "fn main() {}" > cli/src/main.rs \
 && echo "fn main() {}" > relay/src/main.rs \
 && echo "fn main() {}" > gui/src-tauri/src/main.rs \
 && echo "fn main() {}" > gui/src-tauri/build.rs \
 && cargo build --release --locked -p arvolo-relay ${CARGO_JOBS:+-j ${CARGO_JOBS}} \
 && rm -rf core/src cli/src relay/src arvolo-ipc/src gui/src-tauri

# 2) Build the real binary (deps are cached above). Touch the workspace sources
# so cargo recompiles our crates instead of reusing the dummy dep-cache build
# (Docker COPY mtimes can otherwise make cargo think they're unchanged).
COPY . .
RUN find core cli relay arvolo-ipc -name "*.rs" -exec touch {} + \
 && cargo build --release --locked -p arvolo-relay ${CARGO_JOBS:+-j ${CARGO_JOBS}}

FROM debian:trixie-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/* \
 && useradd -r -u 10001 arvolo \
 && mkdir -p /data && chown arvolo:arvolo /data
COPY --from=builder /app/target/release/arvolo-relay /usr/local/bin/arvolo-relay

USER arvolo
ENV ARVOLO_RELAY_ADDR=0.0.0.0:6282 \
    ARVOLO_RELAY_DB=/data/relay.db \
    ARVOLO_RELAY_BLOBS=/data/blobs \
    ARVOLO_RELAY_BLOBSTORE=/data/blobstore
EXPOSE 6282
VOLUME ["/data"]
CMD ["arvolo-relay"]
