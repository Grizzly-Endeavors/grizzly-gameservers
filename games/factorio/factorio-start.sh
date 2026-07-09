#!/bin/sh
# Seed the supervisor's minted RCON password (injected as $RCON_PASSWORD) into
# the file factoriotools' entrypoint reads its password from, then hand off to
# that entrypoint. Factorio's headless server takes its RCON password from
# /factorio/config/rconpw (not an env var), so this is how the game's RCON server
# and the supervisor's RCON client end up sharing the same secret without baking
# it into the image or any Kubernetes object. If RCON_PASSWORD is unset (RCON
# disabled), we leave any existing rconpw alone and let the entrypoint mint its
# own throwaway.
set -eu

if [ -n "${RCON_PASSWORD:-}" ]; then
    mkdir -p /factorio/config
    printf '%s' "$RCON_PASSWORD" > /factorio/config/rconpw
fi

exec /docker-entrypoint.sh "$@"
