# Composite Valheim image: the grizzly supervisor as PID 1, wrapping the lloesche
# valheim-server as its child. Lets /stop, /restart and crash-relaunch act on the
# game *process* in place. See games/minecraft/Dockerfile for the worked reference.
#
# Valheim has no native RCON, so this image bakes in BepInEx + the ValheimRcon mod
# (see the modfetch stage and valheim-seed-rcon.sh) to give the supervisor a real
# console: Gary can run in-game commands and broadcasts, and the update-on-empty
# loop reads the live player count — the same capabilities the RCON games have.
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

# Fetch the ValheimRcon BepInEx plugin at build time, pinned by version + sha256 so
# the baked DLL is reproducible and tamper-evident (it's an unsigned third-party
# artifact, like the game base itself — ADR-003 carve-out). Reuses the pinned rust
# base rather than pulling a second image just for curl/unzip. Bump the version and
# hash together; latest is published on Thunderstore.
# nosemgrep: dockerfile.best-practice.missing-image-version -- reuses the digest-pinned chef stage.
FROM chef AS modfetch
ARG VALHEIM_RCON_VERSION=1.5.1
ARG VALHEIM_RCON_SHA256=0f74ec85f07ec7c1a4c8e97e29ee86b11ed0a186a17d6f698da089ab4f74012a
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl unzip ca-certificates \
    && curl -fsSL -o /tmp/rcon.zip \
        "https://thunderstore.io/package/download/Tristan/ValheimRcon/${VALHEIM_RCON_VERSION}/" \
    && printf '%s  /tmp/rcon.zip\n' "$VALHEIM_RCON_SHA256" | sha256sum -c - \
    && mkdir -p /out && unzip -j /tmp/rcon.zip ValheimRcon.dll -d /out \
    && apt-get clean && rm -rf /var/lib/apt/lists/* /tmp/rcon.zip

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
# Baked ValheimRcon plugin + the hook that installs it. The hook (a lloesche
# PRE_SERVER_RUN_HOOK) runs after BepInEx is set up on the PVC and before each
# server start, dropping the DLL into the plugins dir and seeding its RCON config.
COPY --from=modfetch /out/ValheimRcon.dll /opt/valheim-plugins/ValheimRcon.dll
COPY games/valheim/valheim-seed-rcon.sh /usr/local/bin/valheim-seed-rcon
RUN chmod +x /usr/local/bin/valheim-seed-rcon
# The supervisor launches lloesche's bootstrap (which execs supervisord).
ENV SUPERVISOR_CHILD_CMD=/usr/local/sbin/bootstrap
# Valheim's worlds and config live under /config; scope Gary's file tools there.
ENV SUPERVISOR_DATA_DIR=/config
# Load BepInEx so the baked ValheimRcon plugin runs; lloesche installs BepInExPack
# onto the /config PVC on boot (config in /config/bepinex, plugins in
# /config/bepinex/plugins). Cannot be combined with VALHEIM_PLUS.
ENV BEPINEX=true
# Install + configure the RCON plugin at the correct point in lloesche's start
# sequence (after BepInEx, before the server). See valheim-seed-rcon.sh.
ENV PRE_SERVER_RUN_HOOK=/usr/local/bin/valheim-seed-rcon
# RCON via the ValheimRcon mod: the supervisor speaks the `valheim` dialect (Source
# framing, single-packet replies, `say` broadcasts, `players` for the online count)
# to the mod's loopback port. The seed hook writes this port + the minted password
# (injected as VALHEIM_RCON_PASSWORD) into the plugin's BepInEx config, so RCON
# comes up on exactly what the supervisor authenticates with. The port is
# pod-internal — never added to a NodePort Service.
ENV SUPERVISOR_RCON_PORT=27015
ENV SUPERVISOR_RCON_DIALECT=valheim
ENV SUPERVISOR_RCON_PASSWORD_ENV=VALHEIM_RCON_PASSWORD
# Readiness stays on the log marker: the RCON port binds during mod init, which can
# lead the world actually being joinable, so the log line remains the truthful
# "players can connect now" signal. This marker is what the Valheim server prints
# once it's registered and joinable; tune it here if a Valheim update changes it.
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
