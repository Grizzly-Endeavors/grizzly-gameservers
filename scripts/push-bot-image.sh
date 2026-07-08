#!/usr/bin/env bash
# Build the Discord bot / ops-agent image (root Dockerfile) and push it to the
# in-cluster registry for dev iteration — the hand-pushed `:dev` path the deploy
# chart pins (deploy/values.yaml). The durable path is the gated CI build.
#
# The registry-push mechanics (localhost port-forward, readiness, tool checks)
# live in scripts/lib/registry-push.sh, shared with push-game-image.sh.
#
# The Deployment pins imagePullPolicy: Always on the :dev tag, so the next
# rollout re-pulls. A chart change (e.g. env edit) merged to main is what makes
# Flux roll the Deployment; pushing a new :dev alone does not restart the pod.
#
# Usage: scripts/push-bot-image.sh [tag]   (default: dev)
set -euo pipefail

tag="${1:-dev}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/registry-push.sh
source "$repo_root/scripts/lib/registry-push.sh"

require_tools docker kubectl curl

image="localhost:5000/grizzly-gameservers:${tag}"

echo "==> building $image"
docker build -f "$repo_root/Dockerfile" -t "$image" "$repo_root"

push_through_registry_forward "$image"

echo "==> done: registry.registry.svc.cluster.local:5000/grizzly-gameservers:${tag}"
echo "    merge a chart change (or roll the Deployment) to pull the new image."
