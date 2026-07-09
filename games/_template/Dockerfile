# Composite REPLACE_GAME image: the grizzly supervisor as PID 1, wrapping the
# game's own server process as its child. This is what lets /stop, /restart and
# crash-relaunch act on the game *process* in place — the pod, PVC and Agones
# allocation all survive a restart, so resume is seconds instead of a
# reschedule. See games/minecraft/Dockerfile for a filled-in, worked example
# (RCON wiring, a real upstream base, etc).
#
# The build context is the repository ROOT (the supervisor is a workspace crate):
#
#   docker build -f games/REPLACE_GAME/Dockerfile \
#     -t registry.registry.svc.cluster.local:5000/grizzly-gameservers-REPLACE_GAME:<tag> .
#
# CI gate-signs the resulting image (ADR-020); an unsigned upstream base image
# rides the same Kyverno carve-out as the Agones SDK sidecar (ADR-003).
#
# Built in cargo-chef stages so a source-only change reuses the cached
# dependency build instead of recompiling the whole tree — this builder section
# is game-agnostic, copy it verbatim. `-p grizzly-supervisor` (package scope,
# NOT --bin) is load-bearing: at a virtual workspace root --bin selects every
# member for feature resolution, dragging the bot's native-tls reqwest (and
# openssl-sys) into this image, which the slim builder deliberately can't
# satisfy.

# Builder base pinned by digest (rust:1-slim-trixie, amd64+arm64). Re-verify
# the current digest against the upstream registry before use — this template
# may be copied long after it was last checked.
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

# REPLACE: the game's own upstream base image, pinned by digest — never a
# floating tag. If it's unsigned (true of most upstream game images), it rides
# the Kyverno carve-out the same way itzg/minecraft-server does (ADR-003).
FROM REPLACE_BASE_IMAGE@sha256:REPLACE_DIGEST
COPY --from=builder /app/target/release/grizzly-supervisor /usr/local/bin/grizzly-supervisor
# REPLACE: the command the supervisor launches as its child — the game's own
# start command or script. The supervisor forwards SIGTERM for a graceful
# shutdown/save and relaunches this in place on /restart.
ENV SUPERVISOR_CHILD_CMD=REPLACE_CHILD_CMD
# REPLACE (optional): if the game's data directory isn't /data, set this so
# Gary's file tools (browse_files/read_file/write_file/restore_file) scope to
# the right place. Defaults to /data if unset.
# ENV SUPERVISOR_DATA_DIR=/data
# REPLACE (optional): how the supervisor decides the game is Ready (which gates
# the one-shot Agones /ready call). By default it waits for a TCP connect on
# SUPERVISOR_GAME_PORT — correct for games that listen on TCP (Terraria) or whose
# RCON/query port opens once the game is up (point SUPERVISOR_GAME_PORT at it).
# A UDP-only game that never opens *any* TCP port (Valheim) would never go Ready
# that way, so instead set a substring that appears in the game's "server
# started" log line; readiness fires on the first captured line containing it.
# ENV SUPERVISOR_READY_LOG_PATTERN=REPLACE_READY_MARKER
# REPLACE (optional): raise the readiness probe's give-up (default 10 min) when
# the game's first boot is slow — e.g. a large SteamCMD download must finish
# before the port opens (Satisfactory pulls ~15 GB). Unused on the log-pattern path.
# ENV SUPERVISOR_READINESS_TIMEOUT_SECS=1800
# REPLACE (optional): enable only if the game speaks RCON (or an equivalent
# console protocol the supervisor supports) and Gary's send_command tool should
# be able to issue console commands. See games/minecraft/Dockerfile for a
# worked example (ephemeral password minting, loopback-only port,
# SUPERVISOR_RCON_MINECRAFT dialect flag). SUPERVISOR_RCON_PASSWORD_ENV picks the
# child env var the minted password is injected under (default RCON_PASSWORD;
# Palworld uses ADMIN_PASSWORD). SUPERVISOR_RCON_PASSWORD_MAX_LEN caps the minted
# password for games that limit it (Palworld's admin password maxes at 30 chars).
# ENV SUPERVISOR_RCON_PORT=REPLACE_PORT
# REPLACE (optional): enable the in-game chat → Gary reverse loop. Set the chat
# format for this game (the supervisor's chat-line dialect) and the trigger a
# player types; the endpoint URL and shared token are wired in gameserver.yaml,
# not baked here. Only `minecraft` is supported today — see games/minecraft for a
# worked example. Leave unset to disable in-game triggers for this game.
# ENV SUPERVISOR_CHAT_FORMAT=REPLACE_FORMAT
# ENV SUPERVISOR_CHAT_TRIGGER=@Gary
# REPLACE: does this base image's startup need root (e.g. to chown a fresh data
# volume, like itzg's does)? If not, add `USER REPLACE_UID` here and keep
# gameserver.yaml's pod hardening as-is — rootless is the security baseline's
# default assumption. If it does need root, drop the pod-level hardening in
# gameserver.yaml and document why, matching games/minecraft/gameserver.yaml.
ENTRYPOINT ["/usr/local/bin/grizzly-supervisor"]
