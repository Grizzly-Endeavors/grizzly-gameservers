#!/usr/bin/env bash
# Build a per-game composite image (supervisor + upstream game server) and push
# it to the in-cluster registry for dev iteration.
#
# The registry-push mechanics (localhost port-forward, readiness, tool checks)
# live in scripts/lib/registry-push.sh, shared with push-bot-image.sh.
#
# Iteration is fast: the game Dockerfile uses cargo-chef, so a source-only change
# rebuilds just the workspace crates. The catalog pins imagePullPolicy: Always on
# the :dev tag, so a freshly /create'd (or cold /start'd) server re-pulls.
#
# Usage: scripts/push-game-image.sh [game] [tag]   (defaults: minecraft dev)
set -euo pipefail

game="${1:-minecraft}"
tag="${2:-dev}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/registry-push.sh
source "$repo_root/scripts/lib/registry-push.sh"

require_tools docker kubectl curl

dockerfile="$repo_root/games/$game/Dockerfile"
image="localhost:5000/grizzly-gameservers-${game}:${tag}"

if [[ ! -f "$dockerfile" ]]; then
    echo "no Dockerfile for game '$game' at $dockerfile" >&2
    exit 1
fi

echo "==> building $image"
docker build -f "$dockerfile" -t "$image" "$repo_root"

# Prove the supervisor actually executes inside this game's vendor base before the
# image reaches the registry. One builder produces the binary for every game but
# each game lands it in a different vendor's runtime base, so anything the binary
# needs from that base — a loader, a shared library — is only ever validated here.
# Without this check the failure lands at allocation time as an Agones Unhealthy
# GameServer with nothing in the bot's logs, which reads like a cluster fault
# rather than a bad image. --version is answered before the supervisor reads its
# environment or opens a socket, so it exits cleanly without starting the game.
echo "==> checking the supervisor runs in the built image"
if ! version="$(docker run --rm "$image" --version 2>&1)"; then
    echo "$version" >&2
    echo "" >&2
    echo "the supervisor binary does not run inside $game's base image — not pushing." >&2
    echo "a 'GLIBC_x.yz not found' loader error above means the binary is dynamically" >&2
    echo "linked against a newer libc than the vendor base ships; games/$game/Dockerfile" >&2
    echo "should be building it for x86_64-unknown-linux-musl." >&2
    exit 1
fi
echo "    $version"

push_through_registry_forward "$image"

echo "==> done: registry.registry.svc.cluster.local:5000/grizzly-gameservers-${game}:${tag}"
echo "    /create (or /shutdown then /start) the server to pull the new image."
