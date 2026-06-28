# Composite Minecraft image: the grizzly supervisor as PID 1, wrapping the
# upstream itzg server as its child. This is what lets /stop, /restart and
# crash-relaunch act on the game *process* in place — the pod, PVC and Agones
# allocation all survive a restart, so resume is seconds instead of a reschedule.
#
# The build context is the repository ROOT (the supervisor is a workspace crate):
#
#   docker build -f games/minecraft/Dockerfile \
#     -t registry.registry.svc.cluster.local:5000/grizzly-gameservers-minecraft:<tag> .
#
# CI gate-signs the resulting image (ADR-020); the upstream itzg base is the only
# unsigned layer and rides the same Kyverno carve-out as the Agones SDK sidecar.
#
# Built in cargo-chef stages so a source-only change reuses the cached dependency
# build instead of recompiling the whole tree: `prepare` distills the workspace
# into a dependency recipe, `cook` builds just the deps (a layer that stays cached
# until Cargo.toml/Cargo.lock change), then the final build recompiles only the
# workspace crates. `-p grizzly-supervisor` (package scope, NOT --bin) is
# load-bearing: at a virtual workspace root --bin selects every member for
# feature resolution, dragging the bot's native-tls reqwest (and openssl-sys)
# into this image, which the slim builder deliberately can't satisfy.

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

# Upstream itzg base pinned by digest (java25-jdk); the only unsigned layer, it
# rides the Agones-sidecar Kyverno carve-out (ADR-003). The supervisor runs as
# PID 1 and must fork/signal itzg's setup (chown /data, server bootstrap), which
# itzg performs as root — so no USER is set here. Rootless game-pod hardening is
# a deferred follow-up (tracked with game-image CI signing + the Agones carve-out).
# nosemgrep: dockerfile.security.missing-user-entrypoint
FROM itzg/minecraft-server:java25-jdk@sha256:7e570b40bb7cabff1567d19903e9ba0c4f8cc6547dcc9fe5ac62c484e84c5734
COPY --from=builder /app/target/release/grizzly-supervisor /usr/local/bin/grizzly-supervisor
# The supervisor launches itzg's /start as its child; itzg still reads EULA,
# MEMORY and friends from the pod environment exactly as before.
ENV SUPERVISOR_CHILD_CMD=/start
ENTRYPOINT ["/usr/local/bin/grizzly-supervisor"]
