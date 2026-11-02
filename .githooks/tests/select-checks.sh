#!/usr/bin/env bash
# Table tests for .githooks/lib/select-checks.sh. Run: bash .githooks/tests/select-checks.sh
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/select-checks.sh
. "$here/../lib/select-checks.sh"

failures=0
expect() {
    local input="$1" want="$2" got
    got="$(printf '%b' "$input" | select_checks | tr '\n' ' ' | sed 's/ $//')"
    if [ "$got" != "$want" ]; then
        echo "FAIL: [$input] -> '$got', want '$want'"
        failures=$((failures + 1))
    fi
}

expect 'docs/design/00-overview.md' ''
expect 'README.md\nCLAUDE.md\ncluster/guardrails/rbac.yaml' ''
expect 'deploy/values.yaml\n.github/workflows/deploy.yml' ''
expect 'crates/bot/src/lib.rs' 'pkg:grizzly-gameservers'
expect 'crates/bot/prompts/gary/System.md' 'pkg:grizzly-gameservers'
expect 'crates/supervisor/src/main.rs' 'pkg:grizzly-supervisor'
expect 'crates/bot/src/a.rs\ncrates/supervisor/src/b.rs' 'pkg:grizzly-gameservers pkg:grizzly-supervisor'
expect 'crates/control-api/src/lib.rs' 'workspace'
expect 'games/valheim/gameserver.yaml' 'pkg:grizzly-gameservers'
expect 'Cargo.lock' 'deny workspace'
expect '.githooks/pre-push' 'workspace'
expect 'some/new/thing.rs' 'workspace'

if [ "$failures" -gt 0 ]; then
    echo "$failures select-checks case(s) failed"
    exit 1
fi
echo "select-checks: all cases pass"
