#!/usr/bin/env bash
# Build the Discord bot / ops-agent image (root Dockerfile) and push it to the
# in-cluster registry for dev iteration — the hand-pushed `:dev` path the deploy
# chart pins (deploy/values.yaml). The durable path is the gated CI build.
#
# Pods pull from registry.registry.svc.cluster.local:5000 (cluster-internal DNS,
# plain HTTP). We can't reach that name from the dev box, so we push through a
# localhost port-forward — Docker treats localhost as an insecure registry by
# default, and the registry stores by repository name, so the repo path the pod
# resolves is identical regardless of the push hostname.
#
# The Deployment pins imagePullPolicy: Always on the :dev tag, so the next
# rollout re-pulls. A chart change (e.g. env edit) merged to main is what makes
# Flux roll the Deployment; pushing a new :dev alone does not restart the pod.
#
# Usage: scripts/push-bot-image.sh [tag]   (default: dev)
set -euo pipefail

tag="${1:-dev}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
image="localhost:5000/grizzly-gameservers:${tag}"

echo "==> building $image"
docker build -f "$repo_root/Dockerfile" -t "$image" "$repo_root"

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

echo "==> done: registry.registry.svc.cluster.local:5000/grizzly-gameservers:${tag}"
echo "    merge a chart change (or roll the Deployment) to pull the new image."
