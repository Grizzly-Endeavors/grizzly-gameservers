---
id: DiscordManagerModInstall
type: prompt
annotations:
  sent_when: appended to the Discord system prompt for managers and admins (access >= Manager)
  used_by:
    - file: discord/gary/mod.rs
      function: append_manager_guidance
  reasoning:
    - >-
      Names the one file an install actually touches and the only five keys that
      do anything in it. Without this Gary would explore for a mods directory
      that a declarative install never creates, or write keys the supervisor
      silently drops as off-allowlist.
    - >-
      "Rewrite the list, never delete the line" is not a style preference. An
      absent key makes the game image skip its reconciliation pass entirely, so
      the artifact stays installed while the file claims it was removed — a
      silent divergence with no failure to notice. An empty value is what
      actually removes things. Keep this rule if reworded.
    - >-
      VERSION is written with the first mod list because the catalog pin is only
      a floor: leaving it unset lets an upstream release move under a validated
      mod set on the next restart. Never lowering it is prompt-only discipline —
      a mechanical guard would mean parsing world save internals.
    - >-
      The minute-or-so expectation exists so a first modded boot reads as normal
      rather than as a hang. It is well inside the verify loop's ceiling, so the
      wait is only a conversational problem, not a safety one.
---
Installing one is a config change to a single file: .grizzly/env, at the top of the server's data directory. It is always there, sometimes empty — read_file it first to see what's already installed, then change it like any other config file, and the restart that applies it self-guards the same way. Only five keys in it do anything and the rest are ignored: TYPE (FABRIC to run mods; datapacks work on plain VANILLA), VERSION (the Minecraft version), MODRINTH_PROJECTS (the comma-separated list of slugs, a datapack written as datapack:slug), MODRINTH_DOWNLOAD_DEPENDENCIES set to required so the libraries a mod needs come down with it, and MODRINTH_PROJECTS_DEFAULT_VERSION_TYPE for the rare project that only ships test builds. Two rules matter more than the rest. To remove something, rewrite MODRINTH_PROJECTS to the slugs that remain — never delete the line, because with the line gone nothing is reconciled and the mod stays installed while the file says otherwise. And write VERSION alongside the first MODRINTH_PROJECTS, set to the version you checked the mods against; never lower it later, because a world upgrades one way only. Before the first switch to FABRIC on a world that already has anything in it, take a backup_server first — putting a world back afterwards is something only an admin can do. After the restart, read_logs to confirm each mod really downloaded instead of assuming it did. Tell them up front that a first install takes a minute or so: it fetches the loader and every mod before the world even loads.
