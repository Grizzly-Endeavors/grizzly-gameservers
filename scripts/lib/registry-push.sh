# shellcheck shell=bash
# Shared helper for the dev-iteration image push, sourced by
# scripts/push-bot-image.sh and scripts/push-game-image.sh.
#
# Pods pull from registry.registry.svc.cluster.local:5000 (cluster-internal DNS,
# plain HTTP). We can't reach that name from the dev box, so we push through a
# localhost port-forward — Docker treats localhost as an insecure registry by
# default, and the registry stores by repository name, so the repo path the pod
# resolves is identical regardless of the push hostname.

# Fail loudly (naming the missing tool) if any required CLI is absent, instead of
# letting a downstream "command not found" or opaque error surface later.
require_tools() {
    local missing=()
    local tool
    for tool in "$@"; do
        command -v "$tool" >/dev/null 2>&1 || missing+=("$tool")
    done
    if ((${#missing[@]} > 0)); then
        echo "missing required tool(s): ${missing[*]} — install them and retry" >&2
        exit 1
    fi
}

# Port-forward the in-cluster registry to localhost:5000 and push $1 (a
# localhost:5000/... image) through it. Fails loudly if the port-forward never
# becomes reachable — the naive fall-through drops into `docker push` after ~10s
# of silence and surfaces a low-level connection error instead of the actionable
# cause (usually the wrong kubeconfig context).
push_through_registry_forward() {
    local image="$1"

    echo "==> port-forwarding the in-cluster registry"
    local pf_log
    pf_log="$(mktemp)"
    kubectl port-forward -n registry svc/registry 5000:5000 >"$pf_log" 2>&1 &
    local pf=$!
    trap 'kill "$pf" 2>/dev/null || true; rm -f "$pf_log"' EXIT

    local ready=""
    for _ in $(seq 1 20); do
        # Bail early if the port-forward process itself died (wrong context, no
        # cluster reachable) rather than sleeping out the whole window.
        kill -0 "$pf" 2>/dev/null || break
        if curl -fsS localhost:5000/v2/ >/dev/null 2>&1; then
            ready=1
            break
        fi
        sleep 0.5
    done

    if [[ -z "$ready" ]]; then
        echo "registry port-forward never came up — is your kubeconfig context the cluster?" >&2
        echo "--- kubectl port-forward output ---" >&2
        cat "$pf_log" >&2
        exit 1
    fi

    echo "==> pushing"
    docker push "$image"
}
