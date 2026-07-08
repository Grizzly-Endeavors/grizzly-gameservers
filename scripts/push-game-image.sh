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

push_through_registry_forward "$image"

echo "==> done: registry.registry.svc.cluster.local:5000/grizzly-gameservers-${game}:${tag}"
echo "    /create (or /shutdown then /start) the server to pull the new image."
