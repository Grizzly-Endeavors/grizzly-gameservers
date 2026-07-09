# Composite Terraria image: the grizzly supervisor as PID 1, wrapping the vanilla
# Terraria dedicated server as its child. Lets /stop, /restart and crash-relaunch
# act on the game *process* in place. See games/minecraft/Dockerfile for the
# worked reference.
#
# Vanilla Terraria has no RCON (only the TShock fork exposes a REST API, which the
# supervisor doesn't speak), so this game runs *without* live console commands:
# Gary can still read/write config files and stop/start/restart it, just not issue
# in-game commands. See docs and games/README.md.
#
# The build context is the repository ROOT (the supervisor is a workspace crate):
#
#   docker build -f games/terraria/Dockerfile \
#     -t registry.registry.svc.cluster.local:5000/grizzly-gameservers-terraria:<tag> .
#
# CI gate-signs the resulting image (ADR-020); the upstream ryshe/terraria base is
# the only unsigned layer and rides the Agones-sidecar Kyverno carve-out (ADR-003).

# Builder base pinned by digest (rust:1-slim-trixie, amd64+arm64); bump deliberately.
FROM rust:1-slim-trixie@sha256:6abf73f05806f36362d0ff2722f2250c6153398831edd0455e0e0baa1f78ecc7 AS chef
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential pkg-config \
    && apt-get clean && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef --locked
WORKDIR /app

# nosemgrep: dockerfile.best-practice.missing-image-version -- internal build stage.
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# nosemgrep: dockerfile.best-practice.missing-image-version -- internal build stage.
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json -p grizzly-supervisor
COPY . .
RUN cargo build --release --locked -p grizzly-supervisor

# Upstream vanilla Terraria server pinned by digest (tag: latest). The only
# unsigned layer; rides the Agones-sidecar Kyverno carve-out (ADR-003). Runs as
# root (the base sets no non-root user); the supervisor runs as PID 1 and forks
# the server, so no USER is set and rootless hardening stays deferred (as minecraft).
FROM ryshe/terraria@sha256:b1c89f7f359abfe1171db454101853c3812b581eecd0f4eeabb9e9f77da240ef
# zstd backs the supervisor's whole-/data archive routes (`tar --zstd`) for the S3
# backup/archive/restore flows.
# nosemgrep: dockerfile.best-practice.missing-package-version -- distro-pinned, base is digest-pinned.
RUN apt-get update \
    && apt-get install -y --no-install-recommends zstd \
    && apt-get clean && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/grizzly-supervisor /usr/local/bin/grizzly-supervisor
COPY games/terraria/terraria-start.sh /usr/local/bin/terraria-start
RUN chmod +x /usr/local/bin/terraria-start
# The supervisor launches our wrapper, which seeds a non-interactive autocreate
# config and execs the server directly.
ENV SUPERVISOR_CHILD_CMD=/usr/local/bin/terraria-start
# Keep the world, config and logs together on the /data PVC (the base defaults
# scatter them under /root and /config). Scope Gary's file tools to /data.
ENV SUPERVISOR_DATA_DIR=/data
ENV WORLDPATH=/data/worlds
ENV CONFIGPATH=/data/config
ENV LOGPATH=/data/logs
# Readiness: Terraria's game port is TCP, so the supervisor's default TCP connect
# probe on it is a clean "ready" signal.
ENV SUPERVISOR_GAME_PORT=7777
# No USER: matches the other game images (root at startup, rootless hardening
# deferred, ADR-003). Terraria doesn't itself require root.
# nosemgrep: dockerfile.security.missing-user-entrypoint
ENTRYPOINT ["/usr/local/bin/grizzly-supervisor"]
