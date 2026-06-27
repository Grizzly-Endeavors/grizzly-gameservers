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

FROM rust:1-slim-trixie AS chef
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential pkg-config \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef --locked
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json -p grizzly-supervisor
COPY . .
RUN cargo build --release --locked -p grizzly-supervisor

FROM itzg/minecraft-server:java25-jdk
COPY --from=builder /app/target/release/grizzly-supervisor /usr/local/bin/grizzly-supervisor
# The supervisor launches itzg's /start as its child; itzg still reads EULA,
# MEMORY and friends from the pod environment exactly as before.
ENV SUPERVISOR_CHILD_CMD=/start
ENTRYPOINT ["/usr/local/bin/grizzly-supervisor"]
