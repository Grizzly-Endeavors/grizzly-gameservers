# Composite Factorio image: the grizzly supervisor as PID 1, wrapping the
# factoriotools headless server as its child. This is what lets /stop, /restart
# and crash-relaunch act on the game *process* in place — the pod, PVC and Agones
# allocation all survive a restart, so resume is seconds instead of a reschedule.
# See games/minecraft/Dockerfile for the worked reference.
#
# The build context is the repository ROOT (the supervisor is a workspace crate):
#
#   docker build -f games/factorio/Dockerfile \
#     -t registry.registry.svc.cluster.local:5000/grizzly-gameservers-factorio:<tag> .
#
# CI gate-signs the resulting image (ADR-020); the upstream factoriotools base is
# the only unsigned layer and rides the same Kyverno carve-out as the Agones SDK
# sidecar (ADR-003).
#
# The builder section is game-agnostic (copied verbatim from minecraft):
# cargo-chef stages so a source-only change reuses the cached dependency build.
# `-p grizzly-supervisor` (package scope, NOT --bin) is load-bearing at a virtual
# workspace root — see games/minecraft/Dockerfile for the full rationale.

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

# Upstream factoriotools headless server pinned by digest (tag: stable). The only
# unsigned layer; rides the Agones-sidecar Kyverno carve-out (ADR-003). Its
# entrypoint bootstraps as root (chowns /factorio, drops to the factorio user via
# runuser), and the supervisor runs as PID 1 and must fork/signal it — so, as with
# minecraft, no USER is set here and rootless game-pod hardening stays deferred.
FROM factoriotools/factorio@sha256:d98a1554e7de21c299f263fde85cbec5c577afae2cef656378be8b658e929d6a
# zstd backs the supervisor's whole-/data archive routes (`tar --zstd`) for the S3
# backup/archive/restore flows; the debian-slim base ships tar but not zstd.
# nosemgrep: dockerfile.best-practice.missing-package-version -- distro-pinned, base is digest-pinned.
RUN apt-get update \
    && apt-get install -y --no-install-recommends zstd \
    && apt-get clean && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/grizzly-supervisor /usr/local/bin/grizzly-supervisor
COPY games/factorio/factorio-start.sh /usr/local/bin/factorio-start
RUN chmod +x /usr/local/bin/factorio-start
# The supervisor launches our wrapper, which seeds the minted RCON password into
# the file factoriotools reads before exec'ing that image's own /docker-entrypoint.sh.
ENV SUPERVISOR_CHILD_CMD=/usr/local/bin/factorio-start
# Factorio's data (saves, config, mods) lives under /factorio; scope Gary's file
# tools there.
ENV SUPERVISOR_DATA_DIR=/factorio
# Enable RCON so Gary's send_command tool can issue console commands. Factorio
# speaks the base Source RCON protocol (no Minecraft quirks), on TCP 27015. The
# supervisor mints the password at pod startup and injects it as RCON_PASSWORD;
# our wrapper writes it to /factorio/config/rconpw, which the upstream entrypoint
# passes to `factorio --rcon-password`. The port stays pod-internal — never added
# to a NodePort Service.
ENV SUPERVISOR_RCON_PORT=27015
# Readiness: Factorio opens its RCON TCP port only once the server is hosting, so
# the supervisor's default TCP connect probe on this port is a clean "ready"
# signal (the game port itself is UDP and can't be TCP-probed).
ENV SUPERVISOR_GAME_PORT=27015
# No USER: the upstream entrypoint needs root to chown /factorio and drop to the
# factorio user; rootless hardening is deferred (see the FROM note above, ADR-003).
# nosemgrep: dockerfile.security.missing-user-entrypoint
ENTRYPOINT ["/usr/local/bin/grizzly-supervisor"]
