#!/usr/bin/env bash
# Drive the *deployed* bot's operator harness from this box, so a change can be
# exercised end-to-end without anyone typing into Discord.
#
# The routes only exist on the ClusterIP Service, so everything here runs through
# a `kubectl port-forward`. Discord ids are read from the gitignored repo-root
# .env — this repo is public, and while a channel id isn't a credential it ties a
# personal Discord identity to the project, so it never enters git.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/kube-forward.sh
. "$repo_root/scripts/lib/kube-forward.sh"

NAMESPACE="${GAMESERVERS_NAMESPACE:-game-servers}"
SERVICE="svc/grizzly-gameservers-agent"
REMOTE_PORT=9360
# Not 9360: `just bot-start` runs a local bot bound to that port, and a silent
# collision would quietly send these requests to the wrong Gary — exactly the
# class of confusion this harness exists to eliminate.
LOCAL_PORT="${GAMESERVERS_OPERATOR_LOCAL_PORT:-19360}"
# A 16-round turn against a 2-minute per-completion ceiling can legitimately run
# for minutes; a short timeout would read as a hang.
REQUEST_TIMEOUT=900

usage() {
    cat >&2 <<'USAGE'
Usage: scripts/operator.sh [options] <command>

Commands:
  tool <name> [json-args]   Run one of Gary's tools directly (no model). The default path.
  gary <prompt>             Run a full Gary turn; returns the reply, a tool-call trace,
                            and the turn's token usage and latency.
  tools                     List the tools a tier may call, with their JSON schemas.

Options:
  --access <tier>   read-only | manager | admin   (default: admin)
  --scope <scope>   guild | all                   (default: guild)
  --channel <id>    Override GAMESERVERS_OPERATOR_CHANNEL from .env
  --user <id>       Attribute the action to this Discord user (default: first operator)
  --guild <id>      Override the guild (default: resolved from the channel)
  --confirm         Pre-approve destructive confirmations. Admin-only; posts an audit line.
  --new-session     Start Gary fresh instead of continuing the conversation
  --raw             Print the raw JSON response instead of pretty-printing it

Examples:
  scripts/operator.sh tools --access read-only
  scripts/operator.sh tool list_servers
  scripts/operator.sh tool read_logs '{"name":"mc","lines":50}'
  scripts/operator.sh gary "what's the status of mc?"
USAGE
    exit 1
}

access=""
scope=""
channel=""
user=""
guild=""
confirm=false
new_session=false
raw=false
positional=()

while (($# > 0)); do
    case "$1" in
        --access) access="${2:-}"; shift 2 ;;
        --scope) scope="${2:-}"; shift 2 ;;
        --channel) channel="${2:-}"; shift 2 ;;
        --user) user="${2:-}"; shift 2 ;;
        --guild) guild="${2:-}"; shift 2 ;;
        --confirm) confirm=true; shift ;;
        --new-session) new_session=true; shift ;;
        --raw) raw=true; shift ;;
        -h|--help) usage ;;
        --*) echo "unknown option: $1" >&2; usage ;;
        *) positional+=("$1"); shift ;;
    esac
done

((${#positional[@]} > 0)) || usage
command="${positional[0]}"

require_tools kubectl curl jq

# Same file the binary reads via dotenvy for an off-cluster run.
if [[ -f "$repo_root/.env" ]]; then
    set -a
    # shellcheck disable=SC1091
    . "$repo_root/.env"
    set +a
fi

# Resolve the token: an explicit .env value wins (so this can be pointed at a
# local bot), otherwise read the Secret ESO syncs from OpenBao.
token="${GAMESERVERS_OPERATOR_TOKEN:-}"
if [[ -z "$token" ]]; then
    token="$(kubectl -n "$NAMESPACE" get secret grizzly-gameservers-operator \
        -o jsonpath='{.data.operator_token}' 2>/dev/null | base64 -d 2>/dev/null || true)"
fi
if [[ -z "$token" ]]; then
    cat >&2 <<EOF
no operator token available, so the harness routes aren't reachable.

The bot only mounts /operator/* when GAMESERVERS_OPERATOR_TOKEN is set. Provision it with:
  bao kv put secret/grizzly-platform/gameservers/operator token=\$(openssl rand -hex 32)
then wait for ESO to sync Secret/grizzly-gameservers-operator in $NAMESPACE.
EOF
    exit 1
fi

# `tools` is a plain lookup and needs no Discord context; everything else acts as
# somebody, somewhere, so it needs a channel to report into.
if [[ "$command" != "tools" ]]; then
    channel="${channel:-${GAMESERVERS_OPERATOR_CHANNEL:-}}"
    if [[ -z "$channel" ]]; then
        cat >&2 <<EOF
no channel to post into.

Set GAMESERVERS_OPERATOR_CHANNEL in $repo_root/.env (gitignored — Discord ids must
not enter this repo), or pass --channel <id>. Use the id of a channel Gary is in;
replies and any confirmation card land there.
EOF
        exit 1
    fi
fi

# Build the request body with jq so every value is properly escaped — a prompt
# with a quote in it must not be able to corrupt the JSON.
build_body() {
    local body
    body="$(jq -n --arg channel "$channel" '{channel: $channel}')"
    [[ -n "$guild" ]] && body="$(jq --arg v "$guild" '. + {guild: $v}' <<<"$body")"
    [[ -n "$user" ]] && body="$(jq --arg v "$user" '. + {user: $v}' <<<"$body")"
    [[ -n "$access" ]] && body="$(jq --arg v "$access" '. + {access: $v}' <<<"$body")"
    [[ -n "$scope" ]] && body="$(jq --arg v "$scope" '. + {scope: $v}' <<<"$body")"
    "$confirm" && body="$(jq '. + {confirm: true}' <<<"$body")"

    case "$command" in
        tool)
            local name="${positional[1]:-}"
            local args="${positional[2]:-{\}}"
            [[ -n "$name" ]] || { echo "tool needs a name — try: scripts/operator.sh tools" >&2; return 1; }
            # Validate before --argjson so a malformed argument reports itself
            # rather than surfacing as jq's parser error on an unnamed input.
            if ! jq -e . >/dev/null 2>&1 <<<"$args"; then
                echo "tool arguments must be a JSON object, got: ${args}" >&2
                echo "quote them as one shell word, e.g. '{\"name\":\"mc\",\"lines\":50}'" >&2
                return 1
            fi
            jq --arg tool "$name" --argjson arguments "$args" \
                '. + {tool: $tool, arguments: $arguments}' <<<"$body"
            ;;
        gary)
            local prompt="${positional[*]:1}"
            [[ -n "${prompt// /}" ]] || { echo "gary needs something to say" >&2; return 1; }
            body="$(jq --arg prompt "$prompt" '. + {prompt: $prompt}' <<<"$body")"
            "$new_session" && body="$(jq '. + {new_session: true}' <<<"$body")"
            printf '%s' "$body"
            ;;
        *) echo "unknown command: $command" >&2; return 1 ;;
    esac
}

base="localhost:${LOCAL_PORT}"
forward_and_wait "$NAMESPACE" "$SERVICE" "${LOCAL_PORT}:${REMOTE_PORT}" \
    "${base}/health" "the deployed bot"

# Retry a 503 a few times: it means the Discord gateway hasn't finished
# connecting, which resolves on its own within seconds of a rollout.
request() {
    local method="$1" url="$2" body="${3:-}"
    local attempt response status
    for attempt in 1 2 3; do
        if [[ -n "$body" ]]; then
            response="$(curl -sS --max-time "$REQUEST_TIMEOUT" -w '\n%{http_code}' \
                -X "$method" -H "Authorization: Bearer ${token}" \
                -H 'Content-Type: application/json' -d "$body" "$url")"
        else
            response="$(curl -sS --max-time "$REQUEST_TIMEOUT" -w '\n%{http_code}' \
                -X "$method" -H "Authorization: Bearer ${token}" "$url")"
        fi
        status="$(tail -n1 <<<"$response")"
        [[ "$status" == "503" ]] || break
        echo "==> bot not ready yet (503), retrying (${attempt}/3)" >&2
        sleep 3
    done
    printf '%s' "$response"
}

case "$command" in
    tools)
        url="${base}/operator/tools"
        [[ -n "$access" ]] && url="${url}?access=${access}"
        response="$(request GET "$url")"
        ;;
    tool)
        # Built on its own line, not nested inside the request substitution: a
        # failure in there would be swallowed and we'd POST an empty body.
        body="$(build_body)" || exit 1
        response="$(request POST "${base}/operator/tool" "$body")"
        ;;
    gary)
        body="$(build_body)" || exit 1
        echo "==> asking gary (this can take a couple of minutes)" >&2
        response="$(request POST "${base}/operator/gary" "$body")"
        ;;
    *) usage ;;
esac

status="$(tail -n1 <<<"$response")"
payload="$(sed '$d' <<<"$response")"

if [[ "$status" != "200" ]]; then
    # Extract first, print second. Redirecting jq's stdout to stderr in the same
    # command as `2>/dev/null` would point it at /dev/null instead, and the whole
    # reason for failing here is to say why.
    message="$(jq -r '.error // empty' <<<"$payload" 2>/dev/null || true)"
    echo "request failed (HTTP ${status}): ${message:-$payload}" >&2
    exit 1
fi

if "$raw"; then
    printf '%s\n' "$payload"
    exit 0
fi

case "$command" in
    tools)
        jq -r '.[] | "\(.function.name)\n    \(.function.description | split("\n")[0])\n    args: \(.function.parameters.properties // {} | keys | join(", "))"' <<<"$payload"
        ;;
    tool)
        jq -r '.result' <<<"$payload"
        ;;
    gary)
        jq -r '
            (.tool_calls // []) as $calls
            | (if ($calls | length) > 0 then
                 "--- tools ---\n" + ([$calls[] | "→ \(.name)(\(.arguments))\n  \(.result | gsub("\n"; "\n  "))"] | join("\n")) + "\n"
               else "" end)
            + "--- cost (\(.generations // [] | length) model calls, \(.latency_ms)ms) ---\n"
            + "tokens: in \(.usage.prompt_tokens // "?"), out \(.usage.completion_tokens // "?"), total \(.usage.total_tokens // "?")\n"
            + "--- reply (rounds: \(.rounds)\(if .escalated then ", ESCALATED" else "" end)) ---\n"
            + .reply
        ' <<<"$payload"
        ;;
esac
