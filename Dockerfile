# Multi-stage build for the Discord bot / ops-agent binary.
#
# serenity uses the native-tls (OpenSSL) backend — see Cargo.toml — so the build
# stage needs the OpenSSL headers and a C toolchain (also required by ring), and
# the runtime stage needs the OpenSSL runtime + CA certificates. That rules out a
# fully static distroless image; debian-slim is the smallest sane runtime here.
#
# cargo-chef stages cache the dependency build so a source-only change rebuilds
# only the workspace crates. `-p grizzly-gameservers` (package scope, not --bin)
# confines feature resolution and the build to the bot; see games/minecraft/
# Dockerfile for why that distinction matters at a virtual workspace root.

# Base images pinned by digest for reproducible builds (tags in comments track
# what the digest resolves to; bump deliberately). rust:1-slim-trixie and
# debian:trixie-slim, linux/amd64+arm64 manifest lists.
FROM rust:1-slim-trixie@sha256:6abf73f05806f36362d0ff2722f2250c6153398831edd0455e0e0baa1f78ecc7 AS chef
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential pkg-config libssl-dev \
    && apt-get clean && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef --locked
WORKDIR /app

# nosemgrep: dockerfile.best-practice.missing-image-version -- `chef` is an
# internal build stage, not a registry image; it can't carry a version tag.
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# nosemgrep: dockerfile.best-practice.missing-image-version -- internal stage.
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json -p grizzly-gameservers
COPY . .
RUN cargo build --release --locked -p grizzly-gameservers

FROM debian:trixie-slim@sha256:28de0877c2189802884ccd20f15ee41c203573bd87bb6b883f5f46362d24c5c2
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && apt-get clean && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 1000 --create-home --shell /usr/sbin/nologin appuser
COPY --from=builder /app/target/release/grizzly-gameservers /usr/local/bin/grizzly-gameservers
# The per-game catalog the shim renders per instance. Read-only at runtime;
# GAMESERVERS_CATALOG_DIR defaults to this path.
COPY --from=builder /app/games /usr/local/share/grizzly-gameservers/games
USER 1000
ENTRYPOINT ["/usr/local/bin/grizzly-gameservers"]
