#!/usr/bin/env bash
# Move backups from the flat `backups/<instance>/` layout into the guild-scoped
# `backups/<guild>/<instance>/` one.
#
# The owning guild is read from each artifact's own manifest sidecar, not from the
# instance's Kubernetes labels: backups outlive the instance they came from, so a
# destroyed server has no Service left to ask, and a *reused* name would answer
# with the wrong guild — the exact crossing this migration exists to undo.
#
# Copies are server-side (no bytes through this machine) and each pair is verified
# at the destination before the source is deleted, so an interrupted run leaves
# both copies rather than neither. Re-running is safe: already-migrated keys carry
# the guild segment and are skipped.
#
#   scripts/migrate-backup-keys.sh --dry-run   # report what would move (default)
#   scripts/migrate-backup-keys.sh --apply     # do it
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

NAMESPACE="${GAMESERVERS_NAMESPACE:-game-servers}"
# Defaults mirror the bot's own config loader (crates/bot/src/config.rs); a drift
# here would migrate a different bucket than the one the bot reads.
S3_ENDPOINT="${GAMESERVERS_S3_ENDPOINT:-http://10.0.0.200:7072}"
S3_BUCKET="${GAMESERVERS_S3_BUCKET:-grizzly-gameservers}"
S3_REGION="${GAMESERVERS_S3_REGION:-us-east-1}"
# Mirrors backup::manifest::UNSCOPED_GUILD_SEGMENT — where an artifact whose
# manifest names no guild lands, so it stays out of every real guild's prefix.
UNSCOPED_SEGMENT="_unscoped"

apply=false
case "${1:---dry-run}" in
    --apply) apply=true ;;
    --dry-run) apply=false ;;
    *)
        echo "usage: $(basename "$0") [--dry-run|--apply]" >&2
        exit 2
        ;;
esac

for tool in aws jq; do
    command -v "$tool" >/dev/null || { echo "missing required tool: $tool" >&2; exit 1; }
done

# An explicit .env value wins; otherwise read the credentials ESO already synced
# into the cluster. Only the two S3 keys are lifted out of .env — the file also
# carries Discord ids and tokens that have no business in this shell.
lift_env() {
    [[ -f "$repo_root/.env" ]] || return 0
    sed -n "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*//p" "$repo_root/.env" \
        | tail -n1 | sed 's/^"\(.*\)"$/\1/; s/^'\''\(.*\)'\''$/\1/'
}

: "${GAMESERVERS_S3_ACCESS_KEY:=$(lift_env GAMESERVERS_S3_ACCESS_KEY)}"
: "${GAMESERVERS_S3_SECRET_KEY:=$(lift_env GAMESERVERS_S3_SECRET_KEY)}"

if [[ -z "${GAMESERVERS_S3_ACCESS_KEY:-}" || -z "${GAMESERVERS_S3_SECRET_KEY:-}" ]]; then
    GAMESERVERS_S3_ACCESS_KEY="$(kubectl -n "$NAMESPACE" get secret grizzly-gameservers-s3 \
        -o jsonpath='{.data.s3_access_key}' 2>/dev/null | base64 -d 2>/dev/null || true)"
    GAMESERVERS_S3_SECRET_KEY="$(kubectl -n "$NAMESPACE" get secret grizzly-gameservers-s3 \
        -o jsonpath='{.data.s3_secret_key}' 2>/dev/null | base64 -d 2>/dev/null || true)"
fi

if [[ -z "${GAMESERVERS_S3_ACCESS_KEY:-}" || -z "${GAMESERVERS_S3_SECRET_KEY:-}" ]]; then
    echo "no S3 credentials: set GAMESERVERS_S3_ACCESS_KEY/_SECRET_KEY, put them in .env," >&2
    echo "or make sure the grizzly-gameservers-s3 secret is readable in $NAMESPACE." >&2
    exit 1
fi

export AWS_ACCESS_KEY_ID="$GAMESERVERS_S3_ACCESS_KEY"
export AWS_SECRET_ACCESS_KEY="$GAMESERVERS_S3_SECRET_KEY"
export AWS_DEFAULT_REGION="$S3_REGION"

s3() { aws --endpoint-url "$S3_ENDPOINT" "$@"; }

# Every object under backups/. The old layout is backups/<instance>/<file> (three
# slash-separated parts); the new one carries a guild segment and is left alone.
mapfile -t keys < <(
    s3 s3api list-objects-v2 --bucket "$S3_BUCKET" --prefix "backups/" \
        --query 'Contents[].Key' --output json 2>/dev/null \
        | jq -r '.[]? // empty'
)

moved=0 skipped=0 failed=0
for key in "${keys[@]}"; do
    [[ -n "$key" ]] || continue
    IFS='/' read -r -a parts <<< "$key"
    if [[ ${#parts[@]} -ne 3 ]]; then
        skipped=$((skipped + 1))
        continue
    fi
    instance="${parts[1]}"
    file="${parts[2]}"

    # The manifest sidecar names the owner. Derive it from whichever half of the
    # pair this key is, so tarball and manifest agree on a destination.
    case "$file" in
        *.tar.zst)        manifest_key="backups/$instance/${file%.tar.zst}.manifest.json" ;;
        *.manifest.json)  manifest_key="$key" ;;
        *) skipped=$((skipped + 1)); continue ;;
    esac

    guild="$(s3 s3 cp "s3://$S3_BUCKET/$manifest_key" - 2>/dev/null | jq -r '.guild // empty' || true)"
    if [[ -z "$guild" ]]; then
        # No manifest, or one that names no guild: it cannot be attributed, so it
        # goes to the reserved segment rather than being guessed into a real guild.
        guild="$UNSCOPED_SEGMENT"
        echo "warn: $key has no readable owner; filing under $UNSCOPED_SEGMENT" >&2
    fi

    dest="backups/$guild/$instance/$file"
    if [[ "$apply" != true ]]; then
        echo "would move $key -> $dest"
        moved=$((moved + 1))
        continue
    fi

    if ! s3 s3api copy-object --bucket "$S3_BUCKET" --key "$dest" \
        --copy-source "$S3_BUCKET/$key" >/dev/null 2>&1; then
        echo "error: failed to copy $key -> $dest" >&2
        failed=$((failed + 1))
        continue
    fi
    # Only drop the source once the destination is confirmed present, so an
    # interrupted run leaves a duplicate rather than a hole.
    if ! s3 s3api head-object --bucket "$S3_BUCKET" --key "$dest" >/dev/null 2>&1; then
        echo "error: copy of $key reported success but $dest is absent; source kept" >&2
        failed=$((failed + 1))
        continue
    fi
    s3 s3api delete-object --bucket "$S3_BUCKET" --key "$key" >/dev/null
    echo "moved $key -> $dest"
    moved=$((moved + 1))
done

if [[ "$apply" == true ]]; then
    echo "migrated $moved object(s), skipped $skipped already-scoped, $failed failed"
else
    echo "would migrate $moved object(s); $skipped already scoped. Re-run with --apply."
fi
[[ "$failed" -eq 0 ]]
