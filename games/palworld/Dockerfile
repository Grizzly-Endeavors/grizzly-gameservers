# Composite Palworld image: the grizzly supervisor as PID 1, wrapping the
# thijsvanloef palworld-server-docker entrypoint as its child. Lets /stop,
# /restart and crash-relaunch act on the game *process* in place — the pod, PVC
# and Agones allocation all survive a restart. See games/minecraft/Dockerfile for
# the worked reference.
#
# The build context is the repository ROOT (the supervisor is a workspace crate):
#
#   docker build -f games/palworld/Dockerfile \
#     -t registry.registry.svc.cluster.local:5000/grizzly-gameservers-palworld:<tag> .
#
# CI gate-signs the resulting image (ADR-020); the upstream palworld base is the
# only unsigned layer and rides the Agones-sidecar Kyverno carve-out (ADR-003).
#
# The builder section is game-agnostic (copied verbatim from minecraft): cargo-chef
# stages so a source-only change reuses the cached dependency build. `-p
# grizzly-supervisor` (package scope, NOT --bin) is load-bearing at a virtual
# workspace root — see games/minecraft/Dockerfile for the rationale.

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

# Upstream palworld server pinned by digest (tag: latest). Its entrypoint runs as
# root to usermod/chown, then drops to the steam user via `su` (it refuses root
# only when PUID/PGID are 0 — we set them below), and installs a SIGTERM trap that
# does a graceful RCON save+shutdown. The supervisor runs as PID 1 and forks it, so
# no USER is set here and rootless game-pod hardening stays deferred (as minecraft).
FROM thijsvanloef/palworld-server-docker@sha256:f0769c2d336d00e46c2b5f74f5bf079628c1a454e176a4b31836f98eaaf4d8c4
# zstd backs the supervisor's whole-/data archive routes (`tar --zstd`) for the S3
# backup/archive/restore flows.
# nosemgrep: dockerfile.best-practice.missing-package-version -- distro-pinned, base is digest-pinned.
RUN apt-get update \
    && apt-get install -y --no-install-recommends zstd \
    && apt-get clean && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/grizzly-supervisor /usr/local/bin/grizzly-supervisor
# The supervisor launches the palworld image's own entrypoint as its child.
ENV SUPERVISOR_CHILD_CMD=/home/steam/server/init.sh
# Non-zero PUID/PGID so the entrypoint usermods the steam user and drops to it
# rather than refusing to run under the root-PID-1 supervisor.
ENV PUID=1000
ENV PGID=1000
# Palworld's save data lives under /palworld; scope Gary's file tools there.
ENV SUPERVISOR_DATA_DIR=/palworld
# Enable Palworld's native RCON (base Source protocol, no Minecraft quirks) so
# Gary's send_command tool can issue console commands. Palworld reuses the admin
# password as the RCON password and caps it at 30 characters, so the supervisor
# mints the password, truncates it to 30, and injects it as ADMIN_PASSWORD — the
# same value the game writes to PalWorldSettings.ini and authenticates RCON with.
# The RCON port stays pod-internal (never added to a NodePort Service).
ENV RCON_ENABLED=true
ENV RCON_PORT=25575
ENV SUPERVISOR_RCON_PORT=25575
ENV SUPERVISOR_RCON_PASSWORD_ENV=ADMIN_PASSWORD
ENV SUPERVISOR_RCON_PASSWORD_MAX_LEN=30
# Readiness: Palworld opens its RCON TCP port once the server is up, so the
# supervisor's default TCP connect probe on this port is a clean "ready" signal
# (the game port itself is UDP and can't be TCP-probed).
ENV SUPERVISOR_GAME_PORT=25575
# No USER: the entrypoint needs root to usermod/chown before dropping to steam;
# rootless hardening is deferred (see the FROM note above, ADR-003).
# nosemgrep: dockerfile.security.missing-user-entrypoint
ENTRYPOINT ["/usr/local/bin/grizzly-supervisor"]
