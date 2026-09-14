# Composite Enshrouded image: the grizzly supervisor as PID 1, wrapping the
# Enshrouded dedicated server as its child. Lets /stop, /restart and
# crash-relaunch act on the game *process* in place. See games/minecraft/
# Dockerfile for the worked reference.
#
# Enshrouded's server is a Windows binary with no Linux build, so it runs under
# GE-Proton. The jsknnr image supplies the Proton build, steamcmd and the Wine
# runtime libraries; its entrypoint is replaced by enshrouded-start.sh, which says
# why.
#
# Enshrouded has no RCON or any other console protocol, so this game runs without
# live commands: Gary operates it through enshrouded_server.json and
# stop/start/restart.
#
# The build context is the repository ROOT (the supervisor is a workspace crate):
#
#   docker build -f games/enshrouded/Dockerfile \
#     -t registry.registry.svc.cluster.local:5000/grizzly-gameservers-enshrouded:<tag> .
#
# CI gate-signs the resulting image (ADR-020); the upstream jsknnr base is the
# only unsigned layer and rides the Agones-sidecar Kyverno carve-out (ADR-003).

# Builder base pinned by digest (rust:1-slim-trixie, amd64+arm64); bump deliberately.
FROM rust:1-slim-trixie@sha256:6abf73f05806f36362d0ff2722f2250c6153398831edd0455e0e0baa1f78ecc7 AS chef
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential pkg-config musl-tools \
    && apt-get clean && rm -rf /var/lib/apt/lists/*
WORKDIR /app
# Materialise the toolchain the workspace pins — its channel, and the musl target
# rust-toolchain.toml declares for the statically-linked supervisor — before any
# cargo invocation, so both stages below run the same rustc. Without it `cargo chef
# cook` compiles the dependency layer against the image's default toolchain, the
# pinned one materialises only once the full source lands, and every dependency is
# rebuilt regardless — defeating the cache these stages exist to provide.
COPY rust-toolchain.toml .
RUN rustup toolchain install
RUN cargo install cargo-chef --locked

# nosemgrep: dockerfile.best-practice.missing-image-version -- internal build stage.
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# nosemgrep: dockerfile.best-practice.missing-image-version -- internal build stage.
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --locked --target x86_64-unknown-linux-musl \
    --recipe-path recipe.json -p grizzly-supervisor
COPY . .
RUN cargo build --release --locked --target x86_64-unknown-linux-musl -p grizzly-supervisor

# Upstream jsknnr enshrouded-server pinned by digest (tag: proton-v2.3.2, GE-Proton
# 10-26 on debian:13-slim). The image sets USER steam (uid 10000); this one resets
# it to root so the supervisor runs as PID 1 the way it does in every other game
# image, and enshrouded-start.sh drops the server to steam itself.
FROM sknnr/enshrouded-dedicated-server@sha256:c410b742812cc55723cdaa97950f94cdef1b1f8a332bceedce13da9b681c98ee
USER root
# zstd backs the supervisor's whole-data archive routes (`tar --zstd`) for the S3
# backup/archive/restore flows. tini reaps the Wine processes `proton run` orphans;
# see enshrouded-start.sh.
# nosemgrep: dockerfile.best-practice.missing-package-version -- distro-pinned, base is digest-pinned.
RUN apt-get update \
    && apt-get install -y --no-install-recommends tini zstd \
    && apt-get clean && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/grizzly-supervisor \
     /usr/local/bin/grizzly-supervisor
COPY games/enshrouded/enshrouded-start.sh /usr/local/bin/enshrouded-start
RUN chmod +x /usr/local/bin/enshrouded-start
ENV SUPERVISOR_CHILD_CMD=/usr/local/bin/enshrouded-start
# The whole server install lives on the PVC, not just the saves: it is ~8 GB, and
# keeping it off the volume would re-download all of it on every cold start. The
# config, saves and logs sit beside it, so this is also where Gary's file tools
# find enshrouded_server.json.
ENV SUPERVISOR_DATA_DIR=/home/steam/enshrouded
# The name a fresh instance's enshrouded_server.json is seeded with. Read once, when
# the file doesn't exist yet; the file is authoritative after that.
ENV ENSHROUDED_SEED_NAME="Grizzly Enshrouded"
# Enshrouded listens on one UDP port, the query port, for both the server browser
# and gameplay. A player adds the server to Steam's favourites by that port, and
# the server binds exactly what its config names, so the provisioner leases a band
# port with no remap and injects it here per instance (games/enshrouded/
# service.yaml); this default only serves a run outside the cluster.
ENV ENSHROUDED_QUERY_PORT=15637
# Occupancy over the Steam query protocol on that same port — the server answers
# the browser's A2S_INFO with its player count, and there is no console or API to
# ask instead. The port is injected alongside ENSHROUDED_QUERY_PORT per instance.
ENV SUPERVISOR_OCCUPANCY_API=a2s
ENV SUPERVISOR_A2S_PORT=15637
# Readiness: Enshrouded is UDP-only and opens no TCP port at all, so the connect
# probe can never succeed. This line is logged once the session is online and the
# server is accepting players.
ENV SUPERVISOR_READY_LOG_PATTERN="'HostOnline' (up)!"
# No USER: the start script needs root to chown the data volume before it drops to
# steam; rootless hardening is deferred as for the other game images (ADR-003).
# nosemgrep: dockerfile.security.missing-user-entrypoint
ENTRYPOINT ["/usr/local/bin/grizzly-supervisor"]
