#!/bin/sh
# Launch the vanilla Terraria dedicated server non-interactively as a direct
# child of the supervisor. The upstream image's bootstrap.sh (a) drops into an
# interactive world-selection prompt when no world config is present — which
# hangs forever with the supervisor's null stdin — and (b) runs the server
# without `exec`, so a SIGTERM would hit bash, not the server. This wrapper fixes
# both: it seeds a server config that autocreates a world on first boot, then
# `exec`s the server so the supervisor's stop/restart signals reach it directly.
#
# WORLDPATH / CONFIGPATH / LOGPATH are pointed at the /data PVC by the Dockerfile,
# so the world, config and logs all persist on one volume. amd64 only (the
# cluster is x86_64); the arm mono path the upstream bootstrap carries is dropped.
set -eu

mkdir -p "${WORLDPATH}" "${CONFIGPATH}" "${LOGPATH}"

config="${CONFIGPATH}/serverconfig.txt"
if [ ! -f "$config" ]; then
    # autocreate makes the server generate the world on first boot instead of
    # prompting; every subsequent boot loads the same world file.
    {
        printf 'world=%s/grizzly.wld\n' "${WORLDPATH}"
        printf 'autocreate=2\n'
        printf 'worldname=Grizzly\n'
        printf 'difficulty=0\n'
        printf 'maxplayers=8\n'
        printf 'port=7777\n'
        printf 'password=\n'
    } > "$config"
fi

cd /terraria-server
exec ./TerrariaServer -config "$config" -logpath "${LOGPATH}"
