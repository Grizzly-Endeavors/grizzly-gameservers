#!/usr/bin/env bash
# Fan a local eval run out across this box and the gs66 and tk-dev dev hosts.
#
# Cases are dealt round-robin across the hosts in proportion to their
# concurrency. Each host runs `evals/harness/run.py --local` on its shard with
# the Docker backend, and every host's records are merged into OUT_DIR, which
# is then scored. A failing host is reported and does not stop the others.
# Ctrl-C kills the remote runs and removes the containers they started.
#
# Usage: scripts/eval-local-fanout.sh [options] CASES_DIR OUT_DIR
set -euo pipefail

usage() {
    cat >&2 <<'USAGE'
Usage: scripts/eval-local-fanout.sh [options] CASES_DIR OUT_DIR

  CASES_DIR            case directory inside this repo (e.g. evals/cases/dev)
  OUT_DIR              where the merged records, transcripts and summary land

Options:
  --case-list FILE    case ids to run, one per line ('#' comments allowed);
                      default: every case in CASES_DIR
  --concurrency MAP   per-host concurrency (default: "local=3 gs66=3 tk-dev=2").
                      Hosts left out of the map are not used; a host set to 0 is skipped.
  --dry-run           print the shards and every command; run nothing
USAGE
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly ALL_HOSTS=(local gs66 tk-dev)
readonly REMOTE_ROOT="grizzly-eval"
readonly SSH_OPTS=(-o BatchMode=yes -o ConnectTimeout=15)
readonly RSYNC_SSH="ssh -o BatchMode=yes -o ConnectTimeout=15"

die() {
    printf 'eval-local: %s\n' "$*" >&2
    exit 2
}

note() {
    printf 'eval-local: %s\n' "$*"
}

case_list=""
concurrency_map="local=3 gs66=3 tk-dev=2"
dry_run=0
while (($#)); do
    case "$1" in
        --case-list) case_list="${2:?--case-list needs a file}"; shift 2 ;;
        --concurrency) concurrency_map="${2:?--concurrency needs a map}"; shift 2 ;;
        --dry-run) dry_run=1; shift ;;
        -h|--help) usage; exit 0 ;;
        -*) usage; die "unknown option $1" ;;
        *) break ;;
    esac
done
if (($# != 2)); then
    usage
    exit 2
fi
cases_dir="$1"
out_dir="$2"

declare -A weight=()
for host in "${ALL_HOSTS[@]}"; do
    weight[$host]=0
done
read -ra entries <<<"${concurrency_map//,/ }"
for entry in "${entries[@]}"; do
    name="${entry%%=*}"
    count="${entry#*=}"
    if [[ "$entry" != *=* || ! "$count" =~ ^[0-9]+$ ]]; then
        die "bad --concurrency entry '$entry' (want host=N)"
    fi
    if [[ -z "${weight[$name]+x}" ]]; then
        die "unknown host '$name' (known: ${ALL_HOSTS[*]})"
    fi
    weight[$name]=$count
done
active=()
for host in "${ALL_HOSTS[@]}"; do
    if ((weight[$host] > 0)); then
        active+=("$host")
    fi
done
((${#active[@]} > 0)) || die "no host has a concurrency above 0"

[[ -d "$cases_dir" ]] || die "no such case directory: $cases_dir"
cases_abs="$(cd "$cases_dir" && pwd)"
case "$cases_abs" in
    "$repo_root"/*) ;;
    *) die "CASES_DIR must be inside the repo ($repo_root): remote checkouts only carry the repo" ;;
esac
cases_rel="${cases_abs#"$repo_root"/}"

all_ids_text="$(python3 -P -c '
import glob, os, sys
import yaml
for path in sorted(glob.glob(os.path.join(sys.argv[1], "*.y*ml"))):
    with open(path, encoding="utf-8") as handle:
        print(yaml.safe_load(handle)["id"])
' "$cases_abs")"
[[ -n "$all_ids_text" ]] || die "no cases found in $cases_dir"
mapfile -t all_ids <<<"$all_ids_text"

if [[ -n "$case_list" ]]; then
    [[ -f "$case_list" ]] || die "no such case list: $case_list"
    selected_text="$(sed -e 's/#.*//' -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' "$case_list" | awk 'NF')"
    [[ -n "$selected_text" ]] || die "case list $case_list names no cases"
    mapfile -t ids <<<"$selected_text"
    declare -A known=()
    for id in "${all_ids[@]}"; do
        known[$id]=1
    done
    for id in "${ids[@]}"; do
        [[ -n "${known[$id]+x}" ]] || die "case '$id' from $case_list is not in $cases_dir"
    done
    dupes="$(printf '%s\n' "${ids[@]}" | sort | uniq -d | tr '\n' ' ')"
    [[ -z "$dupes" ]] || die "case list repeats: $dupes"
else
    ids=("${all_ids[@]}")
fi

# Each host appears once per unit of concurrency per round, so the slot list
# interleaves the hosts and case k goes to slot k mod (sum of concurrencies).
slots=()
max_weight=0
for host in "${active[@]}"; do
    if ((weight[$host] > max_weight)); then
        max_weight=${weight[$host]}
    fi
done
for ((round = 1; round <= max_weight; round++)); do
    for host in "${active[@]}"; do
        if ((weight[$host] >= round)); then
            slots+=("$host")
        fi
    done
done
declare -A shard=()
for i in "${!ids[@]}"; do
    host="${slots[$((i % ${#slots[@]}))]}"
    shard[$host]+="${ids[$i]}"$'\n'
done

run_id="gfan-$(date -u +%Y%m%dT%H%M%SZ)-$$"
# Cleanup patterns carry a bracketed first letter so a `pkill -f` or docker
# name filter run from a shell whose own command text names this run cannot
# match itself.
kill_pattern="[${run_id:0:1}]${run_id:1}"
stage="$out_dir/.staging/$run_id"

env_file="$repo_root/.env"
if [[ ! -f "$env_file" ]]; then
    common_dir="$(cd "$repo_root" && git rev-parse --path-format=absolute --git-common-dir)"
    env_file="$(dirname "$common_dir")/.env"
fi
if [[ ! -f "$env_file" ]]; then
    if ((dry_run)); then
        note "warning: no .env at the repo root or the main checkout; a real run stops here"
    else
        die "no .env at $repo_root or in the main checkout (the Fireworks key lives there)"
    fi
fi

bin="$repo_root/target/release/grizzly-gameservers"

exec_or_show() {
    if ((dry_run)); then
        printf '      %q ' "$@"
        printf '\n'
    else
        "$@"
    fi
}

# Runs a bash script on a host. The script travels base64-encoded so the
# remote login shell (bash or fish) never has to parse it.
remote_bash() {
    local host="$1" script="$2" encoded
    if ((dry_run)); then
        printf '      ssh %s bash -c <<script\n' "$host"
        while IFS= read -r line; do
            printf '        %s\n' "$line"
        done <<<"$script"
        return 0
    fi
    encoded="$(printf '%s' "$script" | base64 -w0)"
    # The payload is base64, so expanding it on the client side is the point.
    # shellcheck disable=SC2029
    ssh "${SSH_OPTS[@]}" "$host" "bash -c \"\$(echo $encoded | base64 -d)\"" </dev/null
}

host_case_args() {
    local host="$1" id
    case_args=()
    while IFS= read -r id; do
        if [[ -n "$id" ]]; then
            case_args+=(--case "$id")
        fi
    done <<<"${shard[$host]:-}"
}

run_remote_host() {
    local host="$1" workers="${weight[$1]}"
    host_case_args "$host"
    note "$host: preflight"
    remote_bash "$host" '
command -v docker >/dev/null || { echo "docker is not on PATH" >&2; exit 1; }
command -v rsync >/dev/null || { echo "rsync is not on PATH" >&2; exit 1; }
python3 -c "import yaml" || { echo "python3 cannot import yaml (PyYAML is missing)" >&2; exit 1; }
'
    note "$host: syncing repo, release binary and .env"
    remote_bash "$host" "mkdir -p \"\$HOME/grizzly-eval/bin\" \"\$HOME/grizzly-eval/runs\""
    exec_or_show rsync -a --exclude=/target/ --exclude=/evals/results/ --exclude=/.env --exclude=/.git \
        -e "$RSYNC_SSH" "$repo_root/" "$host:$REMOTE_ROOT/"
    exec_or_show rsync -a -e "$RSYNC_SSH" "$bin" "$host:$REMOTE_ROOT/bin/grizzly-gameservers"
    exec_or_show rsync -a --chmod=F600 -e "$RSYNC_SSH" "$env_file" "$host:$REMOTE_ROOT/.env"
    remote_bash "$host" "chmod 600 \"\$HOME/grizzly-eval/.env\""

    local run_script
    run_script="$(printf '%s\n' \
        "cd \"\$HOME/grizzly-eval\" || exit 1" \
        "export GARY_LOCAL_BIN=\"\$HOME/grizzly-eval/bin/grizzly-gameservers\"" \
        "export EVAL_RUN_PREFIX='$run_id-'" \
        "exec python3 evals/harness/run.py --local --cases $(printf '%q' "$cases_rel") --systems gary-fw-local --concurrency $workers --out \"\$HOME/grizzly-eval/runs/$run_id\" $(printf '%q ' "${case_args[@]}")")"
    note "$host: running ${#case_args[@]} case flags at concurrency $workers"
    remote_bash "$host" "$run_script"
}

run_local_host() {
    local workers="${weight[local]}"
    host_case_args local
    command -v docker >/dev/null || die "docker is not on PATH"
    python3 -P -c "import yaml" || die "python3 cannot import yaml (PyYAML is missing)"
    note "local: running ${#case_args[@]} case flags at concurrency $workers"
    (
        cd "$repo_root"
        exec_or_show env GARY_LOCAL_BIN="$bin" EVAL_RUN_PREFIX="$run_id-" \
            python3 evals/harness/run.py --local --cases "$cases_rel" --systems gary-fw-local \
            --concurrency "$workers" --out "$stage/local" "${case_args[@]}" </dev/null
    )
}

run_host() {
    if [[ "$1" == local ]]; then
        run_local_host
    else
        run_remote_host "$1"
    fi
}

collect_host() {
    local host="$1"
    if [[ "$host" == local ]]; then
        return 0
    fi
    exec_or_show mkdir -p "$stage/$host"
    if ! exec_or_show rsync -a -e "$RSYNC_SSH" "$host:$REMOTE_ROOT/runs/$run_id/" "$stage/$host/"; then
        note "$host: could not collect records (the host may have failed before its run started)"
    fi
}

merge_host() {
    local host="$1" src="$stage/$1"
    if ((dry_run)); then
        note "dry run: would merge $src into $out_dir"
        return 0
    fi
    [[ -d "$src" ]] || return 0
    rsync -a --exclude=manifest.json "$src/" "$out_dir/"
    mkdir -p "$out_dir/manifests"
    if [[ -f "$src/manifest.json" ]]; then
        cp "$src/manifest.json" "$out_dir/manifests/$host.json"
    fi
}

count_records() {
    python3 -P -c '
import collections, glob, json, os, sys
counts = collections.Counter()
for path in glob.glob(os.path.join(sys.argv[1], "*.json")):
    if os.path.basename(path) == "manifest.json":
        continue
    with open(path, encoding="utf-8") as handle:
        counts[json.load(handle).get("outcome", "unknown")] += 1
print(sum(counts.values()), " ".join(f"{k}={v}" for k, v in sorted(counts.items())))
' "$1"
}

cleanup_script="docker ps -aq --filter 'name=^${kill_pattern}-' | xargs -r docker rm -f >/dev/null 2>&1; pkill -f -- '${kill_pattern}' >/dev/null 2>&1; true"

launched=()
declare -A pid_of=()

on_interrupt() {
    trap - INT TERM
    note "interrupted: stopping runs for $run_id and removing their containers"
    local host
    for host in "${launched[@]}"; do
        if [[ "$host" == local ]]; then
            bash -c "$cleanup_script" || true
        else
            remote_bash "$host" "$cleanup_script" || note "$host: cleanup did not complete"
        fi
    done
    local pid
    for pid in "${pid_of[@]}"; do
        kill "$pid" 2>/dev/null || true
    done
    wait 2>/dev/null || true
    exit 130
}

note "run $run_id: ${#ids[@]} cases across ${#active[@]} host(s)"
for host in "${active[@]}"; do
    if [[ -z "${shard[$host]:-}" ]]; then
        note "$host: no cases in its shard, idle"
        continue
    fi
    count=$(grep -c . <<<"${shard[$host]}")
    note "$host: $count case(s), concurrency ${weight[$host]}"
done

if ((dry_run)); then
    note "dry run: nothing is built, synced or run"
    for host in "${active[@]}"; do
        [[ -n "${shard[$host]:-}" ]] || continue
        note "shard $host:"
        while IFS= read -r id; do
            [[ -n "$id" ]] && printf '        %s\n' "$id"
        done <<<"${shard[$host]}"
        run_host "$host"
    done
    for host in "${active[@]}"; do
        [[ -n "${shard[$host]:-}" ]] || continue
        collect_host "$host"
        merge_host "$host"
    done
    exit 0
fi

note "building the release binary"
(cd "$repo_root" && cargo build --release --quiet --bin grizzly-gameservers)
[[ -x "$bin" ]] || die "no binary at $bin after the build"

mkdir -p "$stage"
for host in "${active[@]}"; do
    [[ -n "${shard[$host]:-}" ]] || continue
    printf '%s' "${shard[$host]}" >"$stage/$host.shard"
done

trap on_interrupt INT TERM
for host in "${active[@]}"; do
    [[ -n "${shard[$host]:-}" ]] || continue
    run_host "$host" >"$stage/$host.log" 2>&1 &
    pid_of[$host]=$!
    launched+=("$host")
    note "$host: started (log: $stage/$host.log)"
done

failed=()
for host in "${launched[@]}"; do
    if wait "${pid_of[$host]}"; then
        note "$host: run finished"
    else
        note "$host: FAILED (see $stage/$host.log)"
        failed+=("$host")
    fi
done

for host in "${launched[@]}"; do
    collect_host "$host"
done
for host in "${launched[@]}"; do
    merge_host "$host"
done

note "summary (records in $out_dir):"
total_records=0
for host in "${launched[@]}"; do
    status="ok"
    for failed_host in "${failed[@]}"; do
        [[ "$failed_host" != "$host" ]] || status="FAILED"
    done
    read -r records outcomes <<<"$(count_records "$stage/$host" 2>/dev/null || echo "0 unreadable")"
    total_records=$((total_records + ${records:-0}))
    printf '  %-7s %-7s cases=%-4s records=%-4s %s\n' "$host" "$status" \
        "$(grep -c . <<<"${shard[$host]}")" "${records:-0}" "${outcomes:-}"
done
note "total records: $total_records"

score_failed=0
if ((total_records > 0)); then
    rm -f "$out_dir/eval_results.json"
    # This run has no Claude Code baseline, and the case dir decides the split, so score the
    # records as they are rather than only the scored split.
    if python3 "$repo_root/evals/harness/score.py" "$out_dir" --cases "$cases_abs" \
        --no-baseline --include-dev -o "$out_dir/eval_results.json" >"$out_dir/summary.md"; then
        note "scored: $out_dir/eval_results.json and $out_dir/summary.md"
    else
        note "score.py failed; the records are still in $out_dir"
        score_failed=1
    fi
fi

if ((${#failed[@]} > 0)) || ((score_failed)); then
    note "finished with failures: ${failed[*]:-} score_failed=$score_failed"
    exit 1
fi
note "all hosts finished"
