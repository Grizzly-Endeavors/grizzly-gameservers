# Composite Valheim image: the grizzly supervisor as PID 1, wrapping the lloesche
# valheim-server as its child. Lets /stop, /restart and crash-relaunch act on the
# game *process* in place. See games/minecraft/Dockerfile for the worked reference.
#
# Valheim has no RCON or any console protocol the supervisor speaks, so this game
# runs *without* live console commands: Gary operates it via file edits and
# stop/start/restart, not in-game commands.
#
# The build context is the repository ROOT (the supervisor is a workspace crate):
#
#   docker build -f games/valheim/Dockerfile \
#     -t registry.registry.svc.cluster.local:5000/grizzly-gameservers-valheim:<tag> .
#
# CI gate-signs the resulting image (ADR-020); the upstream lloesche base is the
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

# Upstream lloesche valheim-server pinned by digest (tag: latest). Its bootstrap
# runs as root then execs `tini -- supervisord`, which manages the valheim-server
# process and stops it gracefully (world save) on SIGTERM — tini forwards the
# supervisor's signals. The only unsigned layer; rides the Agones-sidecar Kyverno
# carve-out (ADR-003). No USER: root at startup, rootless hardening deferred.
FROM lloesche/valheim-server@sha256:20fde516ce311e6084f82f295c9eb6934af57b357c657937a04f62bdf5946149
# zstd backs the supervisor's whole-/data archive routes (`tar --zstd`) for the S3
# backup/archive/restore flows.
# nosemgrep: dockerfile.best-practice.missing-package-version -- distro-pinned, base is digest-pinned.
RUN apt-get update \
    && apt-get install -y --no-install-recommends zstd \
    && apt-get clean && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/grizzly-supervisor /usr/local/bin/grizzly-supervisor
# The supervisor launches lloesche's bootstrap (which execs supervisord).
ENV SUPERVISOR_CHILD_CMD=/usr/local/sbin/bootstrap
# Valheim's worlds and config live under /config; scope Gary's file tools there.
ENV SUPERVISOR_DATA_DIR=/config
# Readiness: Valheim is UDP-only and never opens a TCP port the connect probe
# could reach, so readiness is signalled off the server's log instead — this
# marker is what the Valheim server prints once it's registered and joinable.
# Tune it here if a Valheim update changes the wording.
ENV SUPERVISOR_READY_LOG_PATTERN="Game server connected"
# Valheim's graceful save can run past the 90s default; give it room before SIGKILL.
ENV SUPERVISOR_GRACEFUL_TIMEOUT_SECS=120
# Let our supervisor (and the S3 archive flow) own lifecycle and backups: disable
# the lloesche image's own update/restart/backup cron so the two don't fight.
ENV UPDATE_CRON=""
ENV RESTART_CRON=""
ENV BACKUPS=false
# Server identity. SERVER_PASS must be >=5 chars or Valheim refuses to start; this
# is a default join password meant to be changed per server, not a real secret.
ENV SERVER_NAME="Grizzly Valheim"
ENV WORLD_NAME=Grizzly
ENV SERVER_PASS=changeme
# No USER: bootstrap needs root before dropping to the valheim user; rootless
# hardening is deferred (see the FROM note above, ADR-003).
# nosemgrep: dockerfile.security.missing-user-entrypoint
ENTRYPOINT ["/usr/local/bin/grizzly-supervisor"]
