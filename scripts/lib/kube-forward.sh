# shellcheck shell=bash
# Shared helpers for the dev-box scripts that reach into the cluster over a
# port-forward, sourced by scripts/lib/registry-push.sh and scripts/operator.sh.

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

forward_and_wait() {
    local namespace="$1" target="$2" ports="$3" ready_url="$4"
    local what="${5:-$target}"

    echo "==> port-forwarding ${what}"
    local pf_log
    pf_log="$(mktemp)"
    kubectl port-forward -n "$namespace" "$target" "$ports" >"$pf_log" 2>&1 &
    local pf=$!
    # Bake pf/pf_log into the trap now (double quotes) rather than expanding them
    # at EXIT: both are function-local, so under `set -u` the trap would hit them
    # out of scope once this function has returned and abort with "unbound
    # variable" — turning a successful run into a non-zero exit.
    # shellcheck disable=SC2064 # expanding now is the point — see the comment above
    trap "kill ${pf} 2>/dev/null || true; rm -f ${pf_log@Q}" EXIT

    local ready=""
    for _ in $(seq 1 20); do
        kill -0 "$pf" 2>/dev/null || break
        if curl -fsS "$ready_url" >/dev/null 2>&1; then
            ready=1
            break
        fi
        sleep 0.5
    done

    if [[ -z "$ready" ]]; then
        echo "${what} port-forward never came up — is your kubeconfig context the cluster?" >&2
        echo "--- kubectl port-forward output ---" >&2
        cat "$pf_log" >&2
        exit 1
    fi
}
