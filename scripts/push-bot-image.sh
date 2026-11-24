#!/usr/bin/env bash
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
