#!/bin/sh
# Launch the TShock Terraria server non-interactively as a direct child of the
# supervisor. The upstream image's bootstrap.sh (a) drops into an interactive
# world-selection prompt when no world config is present — which hangs forever
# with the supervisor's null stdin — and (b) runs the server without `exec`, so a
# SIGTERM would hit the shell, not the server. This wrapper fixes both: it seeds a
# server config that autocreates a world on first boot, then `exec`s the server so
# the supervisor's stop/restart signals reach it directly.
#
# WORLDPATH / CONFIGPATH / LOGPATH are pointed at the /data PVC by the Dockerfile,
# so the world, config and logs all persist on one volume. amd64 only (the
# cluster is x86_64); the arm mono path the upstream bootstrap carries is dropped.
set -eu

mkdir -p "${WORLDPATH}" "${CONFIGPATH}" "${LOGPATH}"

# Replicated from the upstream bootstrap: TShock loads its API from
# /tshock/ServerPlugins, which the image declares as a volume and is therefore
# empty in a fresh container. Without this the server comes up as plain vanilla
# Terraria — no REST API, so no player count, and none of the reason for being on
# this flavor.
if [ -z "$(ls -A /tshock/ServerPlugins 2>/dev/null)" ]; then
    cp /plugins/* /tshock/ServerPlugins/
fi

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

# TShock's own settings. Its REST API is what gives this game a live player count
# — Terraria has no RCON — and it ships disabled, so it is re-asserted on every
# boot rather than seeded once: a friend or Gary editing config.json must not be
# able to quietly switch off the signal that update-on-empty and Gary's
# check-before-restart both depend on.
#
# The port stays pod-internal. It is never added to the NodePort Service, and the
# game-servers NetworkPolicy blocks pod-to-pod traffic, so the only thing that can
# reach it is the supervisor beside it in this pod.
tshock_config="${CONFIGPATH}/config.json"
rest_port="${SUPERVISOR_TSHOCK_REST_PORT:-7878}"
if [ -f "$tshock_config" ]; then
    scratch="${tshock_config}.next"
    jq --argjson port "$rest_port" \
        '.Settings.RestApiEnabled = true | .Settings.RestApiPort = $port' \
        "$tshock_config" > "$scratch" && mv "$scratch" "$tshock_config"
else
    # TShock fills every unset key from its own defaults and rewrites the file
    # complete on first save, so a two-key seed is enough to bootstrap it.
    printf '{"Settings":{"RestApiEnabled":true,"RestApiPort":%s}}\n' "$rest_port" \
        > "$tshock_config"
fi

cd /tshock
exec ./TShock.Server \
    -configpath "${CONFIGPATH}" \
    -logpath "${LOGPATH}" \
    -config "$config"
