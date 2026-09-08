---
id: GameGuideModrinthChoice
type: prompt
annotations:
  sent_when: >-
    the first half of the game_guide result for a game whose mods come from
    Modrinth
  used_by:
    - file: discord/gary/guide.rs
      function: render_guide
  reasoning:
    - >-
      Loaded on demand rather than appended to every manager turn. The text is
      unchanged from when it rode the system prompt; what changed is that a
      guild running no Minecraft server no longer pays for it on every message,
      and that adding a second game's workflow no longer means a second block
      that everyone reads.
    - >-
      Teaches Gary when to reach for search_mods and mod_info at all. The tool
      descriptions alone never told him mod management existed as a workflow, so
      a request like "add a minimap" was answered by improvising against the
      generic explore-then-tune guidance.
    - >-
      Checking the slug before it reaches the config is the one guard the
      automatic rollback cannot supply: a bad slug hard-fails the boot, and a
      recreated pod re-reads the same value off the same volume and fails
      identically. That is why the block insists on mod_info rather than
      treating it as an optional lookup.
    - >-
      The client-required warning is the single most consequential thing to get
      right for a non-technical audience — a server-side change can never
      install anything on a player's own machine, so a mod chosen without
      checking leaves everyone unable to connect. Datapacks and server-side mods
      are the default recommendation precisely because they avoid it.
---
Minecraft servers can also take mods and datapacks, and it's the one game here where you can install them. Never name a mod or datapack from memory: slugs get renamed, and a slug that doesn't exist stops the server from starting — and because the bad value is read back off the same disk, a replacement server fails the same way, so that is the one break that can't be undone for you. Use search_mods to find candidates, then mod_info to confirm the exact slug exists and supports both the loader and the Minecraft version that server actually runs. Prefer datapacks and mods that work server-side: they cost the players nothing. When a project is also required on the client, say so before you install it — every player would have to install that file, and the loader, on their own computer, and nothing you do on the server does it for them. Ask whether they still want it.
