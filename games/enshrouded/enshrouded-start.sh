#!/bin/bash
# Launch the Enshrouded dedicated server as a well-behaved child of the supervisor.
#
# The upstream jsknnr entrypoint can't be the child as-is, for three reasons this
# wrapper exists to fix:
#
#   - the `proton` launcher can return before the server it spawned exits, so the process
#     the supervisor launched is not the one hosting the game. Upstream holds the
#     container open by watching the query port and exits 0 when it closes, which
#     would report a crash as a clean exit. This script waits on the server process
#     itself and exits non-zero when it dies without being asked to.
#   - Upstream links the server's log file to /proc/1/fd/1. PID 1 here is the
#     supervisor, so the whole game log would bypass the stream the supervisor
#     captures for readiness, Gary's read_logs, and crash preservation. Pointing the
#     link at this script's own stdout instead stops the server from starting at
#     all under Wine, so this script leaves the log a real file on the volume and
#     follows it onto stdout, which is that captured stream.
#   - Upstream rewrites enshrouded_server.json from env on every boot, which would
#     undo every edit Gary makes to the file. This script seeds it once and then
#     re-asserts only the keys the platform owns.
#
# It runs in two phases. As root (how the supervisor launches every game's child)
# it hands the data volume to the steam user — a fresh PVC is root-owned, and a
# restore or a file write through the supervisor lands files as root — then
# re-executes itself under tini as steam. tini is a subreaper, so the Wine
# processes the `proton` launcher orphans are reaped inside this tree instead of piling up
# as zombies under a supervisor that only waits on its own child, and it forwards
# the supervisor's SIGTERM to the second phase.
#
# SIGTERM becomes SIGINT for the server: SIGINT is the signal Enshrouded treats as
# "save and shut down" (it writes the world and exits in a couple of seconds).
set -euo pipefail

timestamp() {
    date +"%Y-%m-%d %H:%M:%S"
}

if [ "$(id -u)" -eq 0 ]; then
    find "$ENSHROUDED_PATH" \! -user steam -exec chown steam:steam {} +
    exec tini -s -- setpriv --reuid=steam --regid=steam --init-groups "$0" "$@"
fi

# Matched by process name, not command line: the `proton` launcher carries the server's
# path in its arguments for the seconds it takes to spawn it, and a command-line
# match would mistake that launcher for the server. Wine names the process after
# the executable, truncated to the kernel's 15-character limit.
server_name='enshrouded_serv'
stopping=0

# shellcheck disable=SC2317  # invoked by trap, not by fallthrough
request_stop() {
    stopping=1
    echo "$(timestamp) grizzly: stop requested, asking the server to save and exit"
    pkill -INT -x "$server_name" || true
}
trap request_stop TERM INT

# `cmd & wait` rather than running in the foreground: bash runs a trap only
# between commands, and `wait` is the one builtin a signal interrupts. A stop that
# lands mid-update (steamcmd can run for minutes) is honoured at once instead of
# after the download, and never goes on to launch the server.
run_interruptibly() {
    "$@" &
    local pid=$!
    until wait "$pid"; do
        if [ "$stopping" -eq 1 ]; then
            kill "$pid" 2> /dev/null || true
            wait "$pid" || true
            echo "$(timestamp) grizzly: stopped before the server started"
            exit 0
        fi
        kill -0 "$pid" 2> /dev/null || return 1
    done
}

pause() {
    sleep "$1" &
    wait $! || true
}

echo "$(timestamp) grizzly: updating the Enshrouded dedicated server"
# A stale app manifest wedges steamcmd in a failed-update state that `validate`
# alone doesn't clear; upstream deletes it before every update for the same reason.
rm -f "$ENSHROUDED_PATH"/steamapps/appmanifest_*.acf
run_interruptibly "$STEAMCMD_PATH/steamcmd.sh" \
    +@sSteamCmdForcePlatformType windows \
    +force_install_dir "$ENSHROUDED_PATH" \
    +login anonymous \
    +app_update "$STEAM_APP_ID" validate \
    +quit
if [ "$stopping" -eq 1 ]; then
    echo "$(timestamp) grizzly: stopped before the server started"
    exit 0
fi

# Seeded once, when the instance has no config yet. Enshrouded has no join
# password of its own: each user group's password is both what a player types to
# join and which permissions they get, and a group with an empty password admits
# anyone who finds the server in the browser with that group's rights. Upstream's
# example ships one such group with every right including kick/ban, so the seed
# replaces it with two locked groups — Admin with every right, Friend without
# kick/ban or access to other players' inventories — each on its own minted
# password. Gary reads them out of this file for whoever should have them.
if [ ! -f "$ENSHROUDED_CONFIG" ]; then
    echo "$(timestamp) grizzly: seeding $ENSHROUDED_CONFIG"
    mint_password() {
        python3 -c 'import secrets; print(secrets.token_urlsafe(12))'
    }
    jq --arg name "$ENSHROUDED_SEED_NAME" \
        --arg admin_password "$(mint_password)" \
        --arg friend_password "$(mint_password)" \
        '.name = $name
         | .userGroups = [
             {name: "Admin", password: $admin_password, canKickBan: true,
              canAccessInventories: true, canEditWorld: true, canEditBase: true,
              canExtendBase: true, reservedSlots: 0},
             {name: "Friend", password: $friend_password, canKickBan: false,
              canAccessInventories: false, canEditWorld: true, canEditBase: true,
              canExtendBase: true, reservedSlots: 0}
           ]' \
        /home/steam/enshrouded_server_example.json > "$ENSHROUDED_CONFIG"
fi

# The keys the platform owns, re-asserted on every boot: where the server listens
# must match the port the Service leases, and saves and logs must stay on the data
# volume. Everything else in the file — name, groups and their passwords, slots,
# game settings — belongs to the instance and is never touched here.
scratch="$ENSHROUDED_CONFIG.next"
jq --argjson port "$ENSHROUDED_QUERY_PORT" \
    '.queryPort = $port
     | .ip = "0.0.0.0"
     | .saveDirectory = "./savegame"
     | .logDirectory = "./logs"' \
    "$ENSHROUDED_CONFIG" > "$scratch"
mv "$scratch" "$ENSHROUDED_CONFIG"

mkdir -p "$ENSHROUDED_PATH/savegame" "$ENSHROUDED_PATH/logs"
log="$ENSHROUDED_PATH/logs/enshrouded_server.log"
# A symlink left by the upstream entrypoint (an instance that once ran on it)
# points at another process's stdout; the server can't write through it.
[ -L "$log" ] && rm -f "$log"
# `-n 0` so the previous launch's lines are never replayed: its "ready" marker
# would otherwise report this launch ready before it is. `-F` follows the file by
# name, so a server that truncates or replaces it at startup is still followed.
tail -n 0 -F "$log" 2> /dev/null &
log_follower=$!
# The follower holds the supervisor's output pipe open; left running past this
# script, it would outlive the launch it belongs to.
trap 'kill "$log_follower" 2> /dev/null || true' EXIT

export WINEDEBUG=-all
proton="$STEAMCMD_PATH/compatibilitytools.d/GE-Proton$GE_PROTON_VERSION/proton"
# The base sets UMU_ID, which GE-Proton 11 reads as "not a Steam game": `run` then
# launches through umu.exe with the lsteamclient bridge disabled, and the server
# hangs before writing a line. So the server is launched with `runinprefix`, which
# hands the exe straight to wine with the bridge in place — but it never builds
# the prefix, and the prefix lives in the container layer, so it is missing on
# every fresh pod. `getcompatpath` runs the full session setup (building or
# upgrading the prefix) and nothing else.
unset UMU_ID
echo "$(timestamp) grizzly: preparing the Proton prefix"
run_interruptibly "$proton" getcompatpath "$ENSHROUDED_PATH" > /dev/null
echo "$(timestamp) grizzly: starting the Enshrouded dedicated server"
"$proton" runinprefix "$ENSHROUDED_PATH/enshrouded_server.exe" &

# The launcher can take several seconds to spawn the server. A stop that arrives
# in that gap found nothing to signal, so it is re-sent once the server exists.
for _ in $(seq 1 120); do
    pgrep -x "$server_name" > /dev/null && break
    pause 1
done
if ! pgrep -x "$server_name" > /dev/null; then
    echo "$(timestamp) grizzly: the server process never appeared after the proton launch" >&2
    exit 1
fi
if [ "$stopping" -eq 1 ]; then
    pkill -INT -x "$server_name" || true
fi

while pgrep -x "$server_name" > /dev/null; do
    pause 2
done

if [ "$stopping" -eq 1 ]; then
    echo "$(timestamp) grizzly: the server saved and exited"
    exit 0
fi
echo "$(timestamp) grizzly: the server exited without being asked to" >&2
exit 1
