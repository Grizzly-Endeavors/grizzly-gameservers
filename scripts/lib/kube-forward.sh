# shellcheck shell=bash
# Shared helpers for the dev-box scripts that reach into the cluster over a
# port-forward, sourced by scripts/lib/registry-push.sh and scripts/operator.sh.

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

# forward_and_wait <namespace> <target> <local:remote> <readiness-url> [what]
#
# Port-forward <target> in <namespace> and block until <readiness-url> answers.
# Fails loudly if it never does — the naive fall-through drops into the real work
# after ~10s of silence and surfaces a low-level connection error instead of the
# actionable cause (usually the wrong kubeconfig context).
#
# Installs an EXIT trap that tears the forward down, so callers don't have to.
forward_and_wait() {
    local namespace="$1" target="$2" ports="$3" ready_url="$4"
    # Separate `local`: an assignment in the same `local` isn't visible to a
    # later one on that line, so `${5:-$target}` there would read as empty.
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
        # Bail early if the port-forward process itself died (wrong context, no
        # cluster reachable) rather than sleeping out the whole window.
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
