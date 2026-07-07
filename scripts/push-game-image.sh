#!/usr/bin/env bash
# Build a per-game composite image (supervisor + upstream game server) and push
# it to the in-cluster registry for dev iteration.
#
# Pods pull from registry.registry.svc.cluster.local:5000 (cluster-internal DNS,
# plain HTTP). We can't reach that name from the dev box, so we push through a
# localhost port-forward — Docker treats localhost as an insecure registry by
# default, and the registry stores by repository name, so the repo path the pod
# resolves is identical regardless of the push hostname.
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
dockerfile="$repo_root/games/$game/Dockerfile"
image="localhost:5000/grizzly-gameservers-${game}:${tag}"

if [[ ! -f "$dockerfile" ]]; then
    echo "no Dockerfile for game '$game' at $dockerfile" >&2
    exit 1
fi

echo "==> building $image"
docker build -f "$dockerfile" -t "$image" "$repo_root"

echo "==> port-forwarding the in-cluster registry"
kubectl port-forward -n registry svc/registry 5000:5000 >/dev/null 2>&1 &
pf=$!
trap 'kill "$pf" 2>/dev/null || true' EXIT
for _ in $(seq 1 20); do
    curl -fsS localhost:5000/v2/ >/dev/null 2>&1 && break
    sleep 0.5
done

echo "==> pushing"
docker push "$image"

echo "==> done: registry.registry.svc.cluster.local:5000/grizzly-gameservers-${game}:${tag}"
echo "    /create (or /shutdown then /start) the server to pull the new image."
