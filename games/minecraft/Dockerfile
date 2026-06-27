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

FROM rust:1-slim-trixie AS builder
# build-essential covers the C toolchain a few transitive build scripts want; the
# supervisor itself pulls no OpenSSL (reqwest is built without a TLS backend — it
# only talks to the loopback Agones SDK).
#
# `-p grizzly-supervisor` (package scope) — NOT `--bin` — is load-bearing: at a
# virtual workspace root `--bin` selects every member for feature resolution, so
# the bot's native-tls reqwest would unify in and drag openssl-sys into this
# build (which this slim image deliberately can't satisfy). `-p` confines feature
# unification to the supervisor's own graph.
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY . .
RUN cargo build --release --locked -p grizzly-supervisor

FROM itzg/minecraft-server:java25-jdk
COPY --from=builder /app/target/release/grizzly-supervisor /usr/local/bin/grizzly-supervisor
# The supervisor launches itzg's /start as its child; itzg still reads EULA,
# MEMORY and friends from the pod environment exactly as before.
ENV SUPERVISOR_CHILD_CMD=/start
ENTRYPOINT ["/usr/local/bin/grizzly-supervisor"]
