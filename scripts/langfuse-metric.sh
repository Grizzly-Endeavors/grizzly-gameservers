#!/usr/bin/env bash
# Run a derivation-layer metric against Langfuse (see docs/operations/data-collection.md on the pre-hackathon branch).
#
# The metrics API takes its query as a URL-encoded JSON blob, which is miserable
# to assemble in a shell by hand — that assembly is the whole point of this
# script, so the documented metrics stay copy-pasteable instead of aspirational.
#
# The bot reaches Langfuse in-cluster and the public hostname hairpins through
# the edge, so this port-forwards the web Service rather than going over the
# internet. It reads the same key pair the bot uses.
#
#   scripts/langfuse-metric.sh escalation-rate [environment] [days]
#   scripts/langfuse-metric.sh cost-per-turn   [environment] [days]
#   scripts/langfuse-metric.sh guardrail       [environment] [days]
#   scripts/langfuse-metric.sh raw '<json query>'
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/kube-forward.sh
. "$repo_root/scripts/lib/kube-forward.sh"

require_tools kubectl curl python3

LANGFUSE_NAMESPACE="${LANGFUSE_NAMESPACE:-langfuse}"
BOT_NAMESPACE="${GAMESERVERS_NAMESPACE:-game-servers}"
# Not 3000: a local dev server on that port would silently swallow these queries.
LOCAL_PORT="${LANGFUSE_LOCAL_PORT:-13000}"

usage() {
    cat >&2 <<'USAGE'
Usage: scripts/langfuse-metric.sh <metric> [environment] [days]
       scripts/langfuse-metric.sh raw '<json query>'

Metrics:
  escalation-rate   avg of the `escalated` score — the experiment's headline number
  escalation-kind   how the escalations split: deliberate handoff vs budget spent
  cost-per-turn     spend per Gary turn, and the token counts behind it
  guardrail         distribution of the `guardrail` verdicts
  volume            turns, model calls and tool calls by environment

Defaults: environment=production, days=30. Pass environment `all` to skip the filter.
USAGE
    exit 1
}

(($# > 0)) || usage
metric="$1"
shift

public_key="${LANGFUSE_PUBLIC_KEY:-}"
secret_key="${LANGFUSE_SECRET_KEY:-}"
if [[ -z "$public_key" || -z "$secret_key" ]]; then
    public_key="$(kubectl -n "$BOT_NAMESPACE" get secret grizzly-gameservers-langfuse \
        -o jsonpath='{.data.langfuse_public_key}' 2>/dev/null | base64 -d 2>/dev/null || true)"
    secret_key="$(kubectl -n "$BOT_NAMESPACE" get secret grizzly-gameservers-langfuse \
        -o jsonpath='{.data.langfuse_secret_key}' 2>/dev/null | base64 -d 2>/dev/null || true)"
fi
if [[ -z "$public_key" || -z "$secret_key" ]]; then
    echo "no langfuse key pair available — is your kubeconfig pointed at the cluster?" >&2
    echo "the pair lives in 1Password as gameservers-langfuse/{public_key,secret_key}." >&2
    exit 1
fi

if [[ "$metric" == "raw" ]]; then
    (($# > 0)) || usage
    query_json="$1"
else
    environment="${1:-production}"
    days="${2:-30}"
    query_json="$(METRIC="$metric" ENVIRONMENT="$environment" DAYS="$days" python3 - <<'PY'
import json, os, sys
from datetime import datetime, timedelta, timezone

metric = os.environ["METRIC"]
environment = os.environ["ENVIRONMENT"]
days = int(os.environ["DAYS"])

now = datetime.now(timezone.utc)
window = {
    "fromTimestamp": (now - timedelta(days=days)).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "toTimestamp": now.strftime("%Y-%m-%dT%H:%M:%SZ"),
}

# Scores posted before the environment fix landed sit in a `default` bucket, and
# operator traffic must never count as product signal — so an explicit
# environment filter is the default rather than an opt-in.
def env_filter():
    if environment == "all":
        return []
    return [{"column": "environment", "operator": "=", "value": environment, "type": "string"}]

if metric == "escalation-rate":
    query = {
        "view": "scores-boolean",
        "metrics": [{"measure": "value", "aggregation": "avg"},
                    {"measure": "count", "aggregation": "count"}],
        "dimensions": [{"field": "name"}],
        "filters": env_filter() + [
            {"column": "name", "operator": "=", "value": "escalated", "type": "string"}],
        **window,
    }
elif metric in ("guardrail", "escalation-kind"):
    # Both are categorical scores keyed by name; only the name differs.
    name = "guardrail" if metric == "guardrail" else "escalation_kind"
    query = {
        "view": "scores-categorical",
        "metrics": [{"measure": "count", "aggregation": "count"}],
        "dimensions": [{"field": "name"}, {"field": "stringValue"}],
        "filters": env_filter() + [
            {"column": "name", "operator": "=", "value": name, "type": "string"}],
        **window,
    }
elif metric in ("cost-per-turn", "volume"):
    # There is no `traces` view on this deployment, so a turn is counted as its
    # root AGENT observation and cost is summed over GENERATION observations.
    query = {
        "view": "observations",
        "metrics": [{"measure": "count", "aggregation": "count"},
                    {"measure": "totalTokens", "aggregation": "sum"},
                    {"measure": "totalCost", "aggregation": "sum"}],
        "dimensions": [{"field": "type"}, {"field": "environment"}],
        "filters": env_filter(),
        **window,
    }
else:
    sys.exit(f"unknown metric: {metric}")

print(json.dumps(query))
PY
)"
fi

forward_and_wait "$LANGFUSE_NAMESPACE" svc/langfuse-web "${LOCAL_PORT}:3000" \
    "http://localhost:${LOCAL_PORT}/api/public/health" "langfuse"

encoded="$(QUERY="$query_json" python3 -c 'import os,urllib.parse;print(urllib.parse.quote(os.environ["QUERY"]))')"
response="$(curl -fsS -u "${public_key}:${secret_key}" \
    "http://localhost:${LOCAL_PORT}/api/public/v2/metrics?query=${encoded}")"

METRIC="$metric" RESPONSE="$response" python3 - <<'PY'
import json, os

metric = os.environ["METRIC"]
rows = json.loads(os.environ["RESPONSE"]).get("data", [])

if not rows:
    print("no data in this window")
    raise SystemExit

if metric == "escalation-rate":
    for row in rows:
        rate, n = row.get("avg_value"), row.get("count_count")
        print(f"escalation rate: {rate:.1%} over {n} scored turns")
elif metric == "cost-per-turn":
    by_type = {row["type"]: row for row in rows if row.get("type")}
    turns = by_type.get("AGENT", {}).get("count_count", 0)
    gen = by_type.get("GENERATION", {})
    cost, tokens = gen.get("sum_totalCost") or 0, gen.get("sum_totalTokens") or 0
    calls = gen.get("count_count", 0)
    if not turns:
        print("no turns in this window")
    else:
        print(f"turns:          {turns}")
        print(f"model calls:    {calls} ({calls / turns:.1f} per turn)")
        print(f"tokens:         {tokens} ({tokens / turns:,.0f} per turn)")
        print(f"total cost:     ${cost:.4f}")
        print(f"cost per turn:  ${cost / turns:.4f}")
        if cost == 0 and tokens:
            print("\ncost is zero with non-zero tokens — the model price is missing, or these")
            print("observations were ingested before it was defined (pricing is not retroactive).")
elif metric in ("guardrail", "escalation-kind"):
    # A distribution reads as shares, not counts: "most escalations are
    # deliberate" is the finding, and raw counts bury it.
    counts = {row.get("stringValue") or "unknown": row.get("count_count", 0) for row in rows}
    total = sum(counts.values())
    for value, count in sorted(counts.items(), key=lambda pair: -pair[1]):
        print(f"{value:<24} {count:>6}  {count / total:.1%}")
    print(f"{'total':<24} {total:>6}")
else:
    print(json.dumps(rows, indent=2))
PY
