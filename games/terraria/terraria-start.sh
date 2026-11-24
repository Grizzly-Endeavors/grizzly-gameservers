#!/bin/sh
set -eu

mkdir -p "${WORLDPATH}" "${CONFIGPATH}" "${LOGPATH}"

if [ -z "$(ls -A /tshock/ServerPlugins 2>/dev/null)" ]; then
    cp /plugins/* /tshock/ServerPlugins/
fi

config="${CONFIGPATH}/serverconfig.txt"
if [ ! -f "$config" ]; then
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

tshock_config="${CONFIGPATH}/config.json"
rest_port="${SUPERVISOR_TSHOCK_REST_PORT:-7878}"
if [ -f "$tshock_config" ]; then
    scratch="${tshock_config}.next"
    jq --argjson port "$rest_port" \
        '.Settings.RestApiEnabled = true | .Settings.RestApiPort = $port' \
        "$tshock_config" > "$scratch" && mv "$scratch" "$tshock_config"
else
    printf '{"Settings":{"RestApiEnabled":true,"RestApiPort":%s}}\n' "$rest_port" \
        > "$tshock_config"
fi

cd /tshock
exec ./TShock.Server \
    -configpath "${CONFIGPATH}" \
    -logpath "${LOGPATH}" \
    -config "$config"
