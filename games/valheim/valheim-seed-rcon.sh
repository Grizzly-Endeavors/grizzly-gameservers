#!/bin/bash
# lloesche PRE_SERVER_RUN_HOOK: runs after BepInEx is set up but before each
# Valheim server start (it blocks the server start until it returns, so the plugin
# is in place before BepInEx's chainloader scans). It does the two things the
# supervisor can't do itself — BepInEx is set up inside this child process, so
# there's no earlier moment the supervisor could reach these dirs:
#
#   1. Drop the baked ValheimRcon plugin into BepInEx's real plugins dir
#      (idempotent; overwrite so a pinned-version bump in the image lands). That
#      dir lives on the image, not the PVC — lloesche only symlinks the *config*
#      dir to /config, not plugins — so the hook re-places it every boot.
#   2. Seed the plugin's RCON port + the supervisor-minted password into its
#      BepInEx config. lloesche symlinks BepInEx/config -> /config/bepinex, so the
#      plugin's cfg lives at /config/bepinex/<guid>.cfg (on the PVC). RCON then
#      comes up on exactly the port/password the supervisor authenticates with.
#      The password is injected into this process's environment by the supervisor
#      (as VALHEIM_RCON_PASSWORD); the port is the supervisor's SUPERVISOR_RCON_PORT.
#
# The whole [1. Rcon] section is rewritten each boot so a password rotated on a new
# pod and a fresh PVC both converge; the plugin re-adds its other config sections
# with defaults via BepInEx Config.Bind on load.
set -eu

plugins_dir=/opt/valheim/bepinex/BepInEx/plugins
config_dir=/config/bepinex
rcon_cfg="${config_dir}/org.tristan.rcon.cfg"
baked_plugin=/opt/valheim-plugins/ValheimRcon.dll

mkdir -p "$plugins_dir" "$config_dir"
cp -f "$baked_plugin" "$plugins_dir/ValheimRcon.dll"

if [ -z "${VALHEIM_RCON_PASSWORD:-}" ]; then
    echo "valheim-seed-rcon: VALHEIM_RCON_PASSWORD is empty; the RCON plugin disables itself" >&2
fi

# Whitelist loopback only: the supervisor connects over 127.0.0.1 and the port is
# never added to a NodePort Service, so this is defense in depth on the mint.
cat > "$rcon_cfg" <<EOF
[1. Rcon]
Port = ${SUPERVISOR_RCON_PORT:-27015}
Password = ${VALHEIM_RCON_PASSWORD:-}
Whitelist IP mask = 127.0.0.1
EOF
