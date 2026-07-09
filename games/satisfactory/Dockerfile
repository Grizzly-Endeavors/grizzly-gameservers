# Composite Satisfactory image: the grizzly supervisor as PID 1, wrapping the
# wolveix satisfactory-server as its child. Lets /stop, /restart and
# crash-relaunch act on the game *process* in place. See games/minecraft/
# Dockerfile for the worked reference.
#
# Satisfactory has no RCON or console protocol the supervisor speaks (only an
# HTTPS admin API), so this game runs *without* live console commands: Gary
# operates it via file edits and stop/start/restart.
#
# A 1.1+ Satisfactory server needs two ports reachable — the game port (7777/udp)
# and a Reliable Messaging port (8888/tcp) — and it advertises both to clients.
# The provisioner leases one edge-band port per port (nodePort == targetPort, no
# remap) and injects the leased numbers via SERVERGAMEPORT / SERVERMESSAGINGPORT
# (which set both the bind and the advertised port in the wolveix image), so the
# server binds and advertises exactly the ports a client dials. The port model is
# declared on the Service annotations (games/satisfactory/service.yaml); the
# SUPERVISOR_GAME_PORT default below is overridden per instance to the leased game
# port so the readiness probe targets it. See issue #29.
#
# The build context is the repository ROOT (the supervisor is a workspace crate):
#
#   docker build -f games/satisfactory/Dockerfile \
#     -t registry.registry.svc.cluster.local:5000/grizzly-gameservers-satisfactory:<tag> .
#
# CI gate-signs the resulting image (ADR-020); the upstream wolveix base is the
# only unsigned layer and rides the Agones-sidecar Kyverno carve-out (ADR-003).

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

# Upstream wolveix satisfactory-server pinned by digest (tag: latest). Its
# entrypoint runs as root to fetch the server via SteamCMD, then drops to the
# steam user via gosu. The only unsigned layer; rides the Agones-sidecar Kyverno
# carve-out (ADR-003). No USER: root at startup, rootless hardening deferred.
FROM wolveix/satisfactory-server@sha256:e103700ae6ae4c50f19dac80eadb2a805c5b885e179ae2a40850e967bf189efd
# zstd backs the supervisor's whole-/data archive routes (`tar --zstd`) for the S3
# backup/archive/restore flows.
# nosemgrep: dockerfile.best-practice.missing-package-version -- distro-pinned, base is digest-pinned.
RUN apt-get update \
    && apt-get install -y --no-install-recommends zstd \
    && apt-get clean && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/grizzly-supervisor /usr/local/bin/grizzly-supervisor
# The supervisor launches the wolveix image's own entrypoint as its child.
ENV SUPERVISOR_CHILD_CMD=/init.sh
# Satisfactory's gamefiles + saves live under /config; scope Gary's file tools there.
ENV SUPERVISOR_DATA_DIR=/config
# Readiness: Satisfactory serves an HTTPS admin API on TCP 7777 once it's up, so
# the supervisor's default TCP connect probe on this port is a clean "ready"
# signal. The first boot downloads ~15 GB via SteamCMD before the port opens, so
# raise the probe's give-up well past the 10-minute default.
ENV SUPERVISOR_GAME_PORT=7777
ENV SUPERVISOR_READINESS_TIMEOUT_SECS=1800
# A little more headroom than the default 4 players.
ENV MAXPLAYERS=8
# No USER: the entrypoint needs root to fetch the server and chown before dropping
# to steam; rootless hardening is deferred (see the FROM note above, ADR-003).
# nosemgrep: dockerfile.security.missing-user-entrypoint
ENTRYPOINT ["/usr/local/bin/grizzly-supervisor"]
