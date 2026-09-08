#!/bin/bash
# lloesche PRE_SERVER_RUN_HOOK: runs after BepInEx is set up but before each
# Valheim server start (it blocks the server start until it returns, so every
# plugin is in place before BepInEx's chainloader scans). It does the three
# things the supervisor can't do itself — BepInEx is set up inside this child
# process, so there is no earlier moment the supervisor could reach these dirs:
#
#   1. Mirror the supervisor's staged mod tree into BepInEx's real directories.
#      The supervisor resolves and stages `.grizzly/mods` into
#      /config/.grizzly/thunderstore/staged/BepInEx before launching this child,
#      but it deliberately knows nothing about where Valheim's loader lives —
#      that knowledge is here, which is what keeps the reconciler game-agnostic.
#      That tree is the *complete* desired state, so plugins/patchers/monomod
#      mirror with --delete: removing a line from the mod list removes the files.
#      config/ mirrors with --ignore-existing so a .cfg a friend or Gary has
#      edited is never clobbered by the package default. core/ is never touched:
#      that is the loader itself, which this image owns.
#   2. Drop the baked ValheimRcon plugin into BepInEx's real plugins dir
#      (idempotent; overwrite so a pinned-version bump in the image lands). That
#      dir lives on the image, not the PVC — lloesche only symlinks the *config*
#      dir to /config, not plugins — so the hook re-places it every boot. It is
#      excluded from the mirror above so --delete can't take it away.
#   3. Seed the plugin's RCON port + the supervisor-minted password into its
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

bepinex_dir=/opt/valheim/bepinex/BepInEx
plugins_dir="${bepinex_dir}/plugins"
config_dir=/config/bepinex
rcon_cfg="${config_dir}/org.tristan.rcon.cfg"
baked_plugin=/opt/valheim-plugins/ValheimRcon.dll
rcon_plugin=ValheimRcon.dll
staged=/config/.grizzly/thunderstore/staged/BepInEx

mkdir -p "$plugins_dir" "$config_dir"

# 1. Mirror the staged mod tree. The supervisor always stages the full directory
# skeleton when the reconciler is on, so this directory is present-and-empty for
# an empty mod list and absent only when the reconciler isn't configured at all —
# in which case the block is skipped and the image behaves as it did before mods
# existed. That distinction is load-bearing: an empty source with --delete is
# exactly what makes deleting a line delete the files.
if [ -d "$staged" ]; then
    for dir in plugins patchers monomod; do
        mkdir -p "${bepinex_dir}/${dir}"
        rsync -a --delete --chmod=D755,F644 --exclude "$rcon_plugin" \
            "${staged}/${dir}/" "${bepinex_dir}/${dir}/"
    done
    # Config is the friend's to own once it exists: a package ships defaults, and
    # re-copying them every boot would silently revert every tuning change. No
    # --delete here for the same reason.
    rsync -a --ignore-existing --chmod=D755,F644 "${staged}/config/" "${config_dir}/"
fi

# 2. The baked RCON plugin, re-placed after the mirror so it survives --delete
# even if the exclude above is ever dropped.
cp -f "$baked_plugin" "$plugins_dir/$rcon_plugin"

# 3. RCON config.
if [ -z "${VALHEIM_RCON_PASSWORD:-}" ]; then
    echo "valheim-pre-server: VALHEIM_RCON_PASSWORD is empty; the RCON plugin disables itself" >&2
fi

# Whitelist loopback only: the supervisor connects over 127.0.0.1 and the port is
# never added to a NodePort Service, so this is defense in depth on the mint.
cat > "$rcon_cfg" <<EOF
[1. Rcon]
Port = ${SUPERVISOR_RCON_PORT:-27015}
Password = ${VALHEIM_RCON_PASSWORD:-}
Whitelist IP mask = 127.0.0.1
EOF
