#!/usr/bin/env bash
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
