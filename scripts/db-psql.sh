#!/usr/bin/env bash
# Open psql against the bot's Postgres database, for the derivation-layer
# queries in scripts/queries/ (see docs/operations/data-collection.md).
#
# Foundation Postgres is a host-network container on the R730xd reached over the
# flat LAN, not a Kubernetes Service — so this needs no port-forward. It also
# can't run from inside the bot pod: that image ships no psql client.
#
# All arguments are passed through to psql, so this works as both an interactive
# shell and a query runner:
#   scripts/db-psql.sh                       # interactive
#   scripts/db-psql.sh -c '\conninfo'
#   scripts/db-psql.sh -f scripts/queries/playtime.sql
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/kube-forward.sh
. "$repo_root/scripts/lib/kube-forward.sh"

require_tools psql

NAMESPACE="${GAMESERVERS_NAMESPACE:-game-servers}"
# Defaults mirror the bot's own config loader (crates/bot/src/config.rs); a drift
# here would point the queries at a different database than the one being written.
DB_HOST="${DB_HOST:-10.0.0.200}"
DB_PORT="${DB_PORT:-5432}"
DB_NAME="${DB_NAME:-grizzly_gameservers}"
DB_USER="${DB_USER:-grizzly_gameservers}"

# An explicit .env value wins, so this can be pointed at a scratch database
# without touching the cluster; otherwise read the credential ESO already synced.
if [[ -z "${DB_PASSWORD:-}" && -f "$repo_root/.env" ]]; then
    # Only lift DB_PASSWORD rather than sourcing .env wholesale — the file also
    # carries Discord ids and tokens that have no business in this shell.
    DB_PASSWORD="$(sed -n 's/^[[:space:]]*DB_PASSWORD[[:space:]]*=[[:space:]]*//p' "$repo_root/.env" \
        | tail -n1 | sed 's/^"\(.*\)"$/\1/; s/^'\''\(.*\)'\''$/\1/')"
fi

if [[ -z "${DB_PASSWORD:-}" ]]; then
    DB_PASSWORD="$(kubectl -n "$NAMESPACE" get secret grizzly-gameservers-db \
        -o jsonpath='{.data.db_password}' 2>/dev/null | base64 -d 2>/dev/null || true)"
fi

if [[ -z "${DB_PASSWORD:-}" ]]; then
    cat >&2 <<'MSG'
no database password available, so the queries can't run.

Set DB_PASSWORD in the repo-root .env, or make sure your kubeconfig points at the
cluster so the ESO-synced secret can be read:

  kubectl -n game-servers get secret grizzly-gameservers-db

The credential itself lives in 1Password as stores-grizzly-gameservers/db_password.
MSG
    exit 1
fi

# PGPASSWORD rather than a password in the URI: the URI shows up in `ps` output
# and in psql's own error messages, and this database is shared.
export PGPASSWORD="$DB_PASSWORD"
exec psql "postgresql://${DB_USER}@${DB_HOST}:${DB_PORT}/${DB_NAME}" "$@"
