#!/usr/bin/env bash
# Push scripts/queries/*.sql into Metabase as saved questions and lay them out
# on a "Gameservers" dashboard.
#
# Why this exists: a dashboard built by clicking lives only inside Metabase's
# database, where it can't be reviewed, grepped, or explained. The SQL in
# scripts/queries/ stays the source of truth — each file carries the caveats
# that make its result trustworthy — and this script is how it reaches the
# browser. Re-run it after editing a query; an existing question is updated in
# place rather than duplicated.
#
# Usage:
#   just seed-metabase              (or: scripts/seed-metabase.sh)
#   scripts/seed-metabase.sh --dry-run
#
# Auth: reads the Metabase API key from 1Password (platform-metabase/api_key),
# or from MB_API_KEY if it is already set.
#
# Prerequisites: the grizzly_gameservers data source must already exist in
# Metabase — that is grizzly-platform's scripts/metabase-add-database.sh, not
# this script's job. See grizzly-platform docs/integration/metabase.md.

set -euo pipefail

DRY_RUN=""
[[ "${1:-}" == "--dry-run" ]] && DRY_RUN=yes

REPO_ROOT="$(git rev-parse --show-toplevel)"
QUERY_DIR="${REPO_ROOT}/scripts/queries"

OP_TOKEN_FILE="${OP_TOKEN_FILE:-${HOME}/.config/op-tokens/operator}"
OP_VAULT="${OP_VAULT:-grizzly-platform}"
NAMESPACE="metabase"
SERVICE="svc/metabase"
LOCAL_PORT="${MB_LOCAL_PORT:-3456}"
DATA_SOURCE="grizzly_gameservers"
COLLECTION_NAME="Grizzly Gameservers"
DASHBOARD_NAME="Gameservers"

# Metabase's dashboard grid is 24 columns wide; two half-width cards per row.
CARD_W=12
CARD_H=8

PF_PID=""

log() { printf '[%s] %s\n' "$(basename "$0")" "$*"; }
die() { log "ERROR: $*" >&2; exit 1; }

# shellcheck disable=SC2317  # invoked by trap, not by fallthrough
cleanup() {
  [[ -n "${PF_PID}" ]] && kill "${PF_PID}" 2>/dev/null
  return 0
}
trap cleanup EXIT

command -v jq >/dev/null || die "jq not on PATH"
command -v curl >/dev/null || die "curl not on PATH"
[[ -d "${QUERY_DIR}" ]] || die "no query directory at ${QUERY_DIR}"

# --- turn a psql script into portable SQL -----------------------------------

# The query files are written for psql and two of them use `\set` plus :'name'
# interpolation. Metabase's native editor speaks SQL only, so the assignments
# are inlined here. Everything else (::date casts, CTEs, window functions) goes
# through untouched.
#
# This is a substitution, not a rewrite: if a query ever needs a value chosen at
# run time, it should become a Metabase template variable in the source file
# rather than growing a second dialect here.
render_query() {
  awk '
    /^\\set[ \t]+/ {
      name = $2
      s = index($0, "\047")
      if (s > 0) {
        rest = substr($0, s + 1)
        e = index(rest, "\047")
        if (e > 0) vars[name] = substr(rest, 1, e - 1)
      }
      next
    }
    {
      for (n in vars) {
        gsub(":\047" n "\047", "\047" vars[n] "\047", $0)
      }
      print
    }
  ' "$1"
}

# The leading `--` comment block becomes the question description, so the
# caveats travel with the number instead of staying behind in the repo.
render_description() {
  awk '
    /^--/ { sub(/^-- ?/, ""); lines = lines $0 "\n"; next }
    /^[[:space:]]*$/ { if (lines == "") next }
    { exit }
    END { printf "%s", lines }
  ' "$1"
}

# active-users.sql -> "Active users"
render_name() {
  basename "$1" .sql | tr '-' ' ' | awk '{ $1 = toupper(substr($1,1,1)) substr($1,2); print }'
}

# --- credentials ------------------------------------------------------------

read_op_field() {
  local item="$1" field="$2" token
  [[ -r "${OP_TOKEN_FILE}" ]] || die "no 1Password service token at ${OP_TOKEN_FILE}"
  token="$(cat "${OP_TOKEN_FILE}")"
  OP_SERVICE_ACCOUNT_TOKEN="${token}" \
    op item get "${item}" --vault "${OP_VAULT}" --fields "${field}" --reveal 2>/dev/null
}

QUERY_FILES=()
while IFS= read -r f; do QUERY_FILES+=("$f"); done < <(find "${QUERY_DIR}" -maxdepth 1 -name '*.sql' | sort)
[[ ${#QUERY_FILES[@]} -gt 0 ]] || die "no .sql files in ${QUERY_DIR}"

if [[ -n "${DRY_RUN}" ]]; then
  log "would seed ${#QUERY_FILES[@]} question(s) into '${COLLECTION_NAME}':"
  for f in "${QUERY_FILES[@]}"; do
    NAME="$(render_name "${f}")"
    log "  - ${NAME}  (from $(basename "${f}"))"
  done
  exit 0
fi

if [[ -z "${MB_API_KEY:-}" ]]; then
  command -v op >/dev/null || die "op not on PATH and MB_API_KEY is unset"
  MB_API_KEY="$(read_op_field platform-metabase api_key)"
fi
[[ -n "${MB_API_KEY}" ]] || die "Metabase API key is empty"

# --- reach metabase ---------------------------------------------------------

# Port-forwarding the Service bypasses the Authentik forward-auth gate on the
# public hostname, which would answer an API call with a sign-in redirect.
command -v kubectl >/dev/null || die "kubectl not on PATH"
kubectl -n "${NAMESPACE}" get "${SERVICE}" >/dev/null 2>&1 \
  || die "${SERVICE} not found in namespace ${NAMESPACE} — is Metabase deployed?"

kubectl -n "${NAMESPACE}" port-forward "${SERVICE}" "${LOCAL_PORT}:80" >/dev/null 2>&1 &
PF_PID=$!

MB="http://127.0.0.1:${LOCAL_PORT}"
READY=""
for _ in $(seq 1 30); do
  if curl -sf "${MB}/api/health" >/dev/null 2>&1; then
    READY=yes
    break
  fi
  sleep 1
done
[[ -n "${READY}" ]] || die "Metabase did not answer on ${MB} — check the pod is Ready"

api() {
  local method="$1" path="$2"
  shift 2
  curl -sS -X "${method}" "${MB}${path}" \
    -H "X-API-Key: ${MB_API_KEY}" \
    -H 'Content-Type: application/json' "$@"
}

# --- locate the data source and collection ----------------------------------

DATABASES="$(api GET /api/database)"
DB_ID="$(printf '%s' "${DATABASES}" | jq -r --arg n "${DATA_SOURCE}" '.data[]? | select(.name == $n) | .id')"
[[ -n "${DB_ID}" ]] \
  || die "no '${DATA_SOURCE}' data source in Metabase — run grizzly-platform's scripts/metabase-add-database.sh postgres ${DATA_SOURCE}"

COLLECTIONS="$(api GET /api/collection)"
COLLECTION_ID="$(printf '%s' "${COLLECTIONS}" | jq -r --arg n "${COLLECTION_NAME}" 'map(select(.name == $n)) | .[0].id // empty')"
if [[ -z "${COLLECTION_ID}" ]]; then
  CREATED="$(api POST /api/collection --data "$(jq -n --arg n "${COLLECTION_NAME}" \
    '{name: $n, description: "Questions seeded from grizzly-gameservers scripts/queries/ — edit the SQL there, not here."}')")"
  COLLECTION_ID="$(printf '%s' "${CREATED}" | jq -r '.id // empty')"
  [[ -n "${COLLECTION_ID}" ]] || die "could not create collection '${COLLECTION_NAME}'"
  log "created collection '${COLLECTION_NAME}' (${COLLECTION_ID})"
fi

EXISTING_ITEMS="$(api GET "/api/collection/${COLLECTION_ID}/items?models=card")"

# --- seed the questions -----------------------------------------------------

CARD_IDS=()
for f in "${QUERY_FILES[@]}"; do
  NAME="$(render_name "${f}")"
  SQL="$(render_query "${f}")"
  DESCRIPTION="$(render_description "${f}")"

  PAYLOAD="$(jq -n \
    --arg name "${NAME}" \
    --arg description "${DESCRIPTION}" \
    --arg sql "${SQL}" \
    --argjson db "${DB_ID}" \
    --argjson collection "${COLLECTION_ID}" \
    '{
       name: $name,
       description: $description,
       collection_id: $collection,
       display: "table",
       visualization_settings: {},
       dataset_query: {
         type: "native",
         database: $db,
         native: { query: $sql, "template-tags": {} }
       }
     }')"

  CARD_ID="$(printf '%s' "${EXISTING_ITEMS}" | jq -r --arg n "${NAME}" '.data[]? | select(.name == $n) | .id' | head -1)"

  if [[ -n "${CARD_ID}" ]]; then
    RESPONSE="$(api PUT "/api/card/${CARD_ID}" --data "${PAYLOAD}")"
    ACTION="updated"
  else
    RESPONSE="$(api POST /api/card --data "${PAYLOAD}")"
    ACTION="created"
  fi

  CARD_ID="$(printf '%s' "${RESPONSE}" | jq -r '.id // empty')"
  if [[ -z "${CARD_ID}" ]]; then
    REASON="$(printf '%s' "${RESPONSE}" | jq -r '.message // .')"
    die "Metabase refused the question '${NAME}': ${REASON}"
  fi

  CARD_IDS+=("${CARD_ID}")
  log "${ACTION} question '${NAME}' (${CARD_ID})"
done

# --- assemble the dashboard -------------------------------------------------

DASH_ITEMS="$(api GET "/api/collection/${COLLECTION_ID}/items?models=dashboard")"
DASH_ID="$(printf '%s' "${DASH_ITEMS}" | jq -r --arg n "${DASHBOARD_NAME}" '.data[]? | select(.name == $n) | .id' | head -1)"
if [[ -z "${DASH_ID}" ]]; then
  CREATED="$(api POST /api/dashboard --data "$(jq -n --arg n "${DASHBOARD_NAME}" --argjson c "${COLLECTION_ID}" \
    '{name: $n, collection_id: $c, description: "Seeded by grizzly-gameservers scripts/seed-metabase.sh. Layout is regenerated on every run — arrange cards in a copy if you want them to stay put."}')")"
  DASH_ID="$(printf '%s' "${CREATED}" | jq -r '.id // empty')"
  [[ -n "${DASH_ID}" ]] || die "could not create dashboard '${DASHBOARD_NAME}'"
  log "created dashboard '${DASHBOARD_NAME}' (${DASH_ID})"
fi

# The layout is rebuilt from scratch each run: dashcards carry negative ids to
# mean "new", and any card omitted from this list is removed from the dashboard.
# That keeps the dashboard a pure function of scripts/queries/ — and is why the
# dashboard description tells anyone rearranging it to work in a copy.
DASHCARDS="$(printf '%s\n' "${CARD_IDS[@]}" | jq -R 'tonumber' | jq -s \
  --argjson w "${CARD_W}" --argjson h "${CARD_H}" \
  'to_entries | map({
     id: (-1 - .key),
     card_id: .value,
     row: ((.key / 2 | floor) * $h),
     col: (if .key % 2 == 0 then 0 else $w end),
     size_x: $w,
     size_y: $h,
     parameter_mappings: [],
     series: []
   })')"

RESPONSE="$(api PUT "/api/dashboard/${DASH_ID}" --data "$(jq -n --argjson d "${DASHCARDS}" '{dashcards: $d}')")"
PLACED="$(printf '%s' "${RESPONSE}" | jq -r '.dashcards | length // empty')"
if [[ -z "${PLACED}" || "${PLACED}" == "null" ]]; then
  REASON="$(printf '%s' "${RESPONSE}" | jq -r '.message // .')"
  die "Metabase refused the dashboard layout: ${REASON}"
fi

log "dashboard '${DASHBOARD_NAME}' now shows ${PLACED} card(s)"
log "open https://analytics.grizzly-endeavors.com/dashboard/${DASH_ID}"
