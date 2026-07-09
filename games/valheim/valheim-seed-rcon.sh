#!/bin/bash
# lloesche PRE_SERVER_RUN_HOOK: runs after BepInEx is installed but before each
# Valheim server start. It does the two things the supervisor can't do itself —
# BepInEx only lands on the /config PVC once lloesche's bootstrap has run, and that
# bootstrap runs *inside* this child process, so there's no earlier moment the
# supervisor could reach the plugins/config dirs:
#
#   1. Drop the baked ValheimRcon plugin into the BepInEx plugins dir (idempotent;
#      overwrite so a pinned-version bump in the image actually lands).
#   2. Seed the plugin's RCON port + the supervisor-minted password into its
#      BepInEx config, so RCON comes up on exactly the port/password the supervisor
#      authenticates with. The password is injected into this process's environment
#      by the supervisor (as VALHEIM_RCON_PASSWORD); the port is the supervisor's
#      own SUPERVISOR_RCON_PORT.
#
# The whole [1. Rcon] section is rewritten each boot so a password rotated on a new
# pod and a fresh PVC both converge; the plugin re-adds its other config sections
# with defaults via BepInEx Config.Bind on load.
set -eu

plugins_dir=/config/bepinex/plugins
config_dir=/config/bepinex/config
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
