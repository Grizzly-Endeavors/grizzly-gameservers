# shellcheck shell=bash
# Shared helper for the dev-iteration image push, sourced by
# scripts/push-bot-image.sh and scripts/push-game-image.sh.
#
# Pods pull from registry.registry.svc.cluster.local:5000 (cluster-internal DNS,
# plain HTTP). We can't reach that name from the dev box, so we push through a
# localhost port-forward — Docker treats localhost as an insecure registry by
# default, and the registry stores by repository name, so the repo path the pod
# resolves is identical regardless of the push hostname.

# shellcheck source=scripts/lib/kube-forward.sh
. "$(dirname "${BASH_SOURCE[0]}")/kube-forward.sh"

# Port-forward the in-cluster registry to localhost:5000 and push $1 (a
# localhost:5000/... image) through it.
push_through_registry_forward() {
    local image="$1"

    forward_and_wait registry svc/registry 5000:5000 localhost:5000/v2/ "the in-cluster registry"

    echo "==> pushing"
    docker push "$image"
}
