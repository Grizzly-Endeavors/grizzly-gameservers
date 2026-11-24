#!/bin/bash
set -eu

bepinex_dir=/opt/valheim/bepinex/BepInEx
plugins_dir="${bepinex_dir}/plugins"
config_dir=/config/bepinex
rcon_cfg="${config_dir}/org.tristan.rcon.cfg"
baked_plugin=/opt/valheim-plugins/ValheimRcon.dll
rcon_plugin=ValheimRcon.dll
patchers_dir="${bepinex_dir}/patchers"
baked_patcher=/opt/valheim-patchers/GrizzlyPlayerInfoGuard.dll
grizzly_patcher=GrizzlyPlayerInfoGuard.dll
staged=/config/.grizzly/thunderstore/staged/BepInEx

mkdir -p "$plugins_dir" "$patchers_dir" "$config_dir"

if [ -d "$staged" ]; then
    for dir in plugins patchers monomod; do
        mkdir -p "${bepinex_dir}/${dir}"
        rsync -a --delete --chmod=D755,F644 \
            --exclude "$rcon_plugin" --exclude "$grizzly_patcher" \
            "${staged}/${dir}/" "${bepinex_dir}/${dir}/"
    done
    rsync -a --ignore-existing --chmod=D755,F644 "${staged}/config/" "${config_dir}/"
fi

cp -f "$baked_plugin" "$plugins_dir/$rcon_plugin"

cp -f "$baked_patcher" "$patchers_dir/$grizzly_patcher"

if [ -z "${VALHEIM_RCON_PASSWORD:-}" ]; then
    echo "valheim-pre-server: VALHEIM_RCON_PASSWORD is empty; the RCON plugin disables itself" >&2
fi

cat > "$rcon_cfg" <<EOF
[1. Rcon]
Port = ${SUPERVISOR_RCON_PORT:-27015}
Password = ${VALHEIM_RCON_PASSWORD:-}
Whitelist IP mask = 127.0.0.1
EOF
