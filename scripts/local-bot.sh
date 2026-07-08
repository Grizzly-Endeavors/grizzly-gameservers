#!/usr/bin/env bash
# Start/stop the grizzly-gameservers Discord bot locally for manual testing.
#
# Secrets (DISCORD_BOT_TOKEN, DISCORD_GUILD_ID, GAMESERVERS_ADMIN_USER_IDS, ...)
# are read from the repo-root .env by the binary itself via dotenvy. This script
# only overrides GAMESERVERS_CATALOG_DIR to the in-repo games/ directory, since
# the compiled default points at the in-container path. The bot talks to the
# cluster in your current kubeconfig context.
#
# Usage: scripts/local-bot.sh {start|stop|restart|status|logs}
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
pid_file="$repo_root/target/local-bot.pid"
log_file="$repo_root/target/local-bot.log"
bin="$repo_root/target/debug/grizzly-gameservers"

running() {
    [[ -f "$pid_file" ]] && kill -0 "$(cat "$pid_file")" 2>/dev/null
}

start() {
    if running; then
        echo "bot already running (pid $(cat "$pid_file"))"
        return 0
    fi
    cd "$repo_root"
    cargo build
    RUST_LOG="${RUST_LOG:-info}" \
        GAMESERVERS_CATALOG_DIR="${GAMESERVERS_CATALOG_DIR:-$repo_root/games}" \
        nohup "$bin" >"$log_file" 2>&1 &
    echo $! >"$pid_file"
    echo "bot started (pid $(cat "$pid_file"))"
    echo "logs: $log_file  (scripts/local-bot.sh logs to follow)"
}

stop() {
    if ! running; then
        echo "bot not running"
        rm -f "$pid_file"
        return 0
    fi
    local pid
    pid="$(cat "$pid_file")"
    kill "$pid"
    rm -f "$pid_file"
    echo "bot stopped (pid $pid)"
}

case "${1:-}" in
    start) start ;;
    stop) stop ;;
    restart)
        stop || true
        start
        ;;
    status)
        if running; then echo "running (pid $(cat "$pid_file"))"; else echo "stopped"; fi
        ;;
    logs)
        if [ ! -f "$log_file" ]; then
            echo "no log file yet at $log_file — run '$0 start' first" >&2
            exit 1
        fi
        tail -f "$log_file"
        ;;
    *)
        echo "usage: $0 {start|stop|restart|status|logs}" >&2
        exit 2
        ;;
esac
