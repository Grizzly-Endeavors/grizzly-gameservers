#!/bin/sh
set -eu

if [ -n "${RCON_PASSWORD:-}" ]; then
    mkdir -p /factorio/config
    printf '%s' "$RCON_PASSWORD" > /factorio/config/rconpw
fi

exec /docker-entrypoint.sh "$@"
