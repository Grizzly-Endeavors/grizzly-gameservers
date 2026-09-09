---
id: ImportProfile
type: tool
tool_schema:
  name:
    type: string
    description: Exact server name, as shown by `list_servers`.
  code:
    type: string
    description: >-
      The profile code the user gave you, exactly as they wrote it — a UUID like
      `01a0838e-8316-f62c-48aa-7bdd59d3f8da`, copied out of r2modman or
      Thunderstore Mod Manager. Never invent one or reuse one from earlier in
      the conversation unless they asked for that one again.
annotations:
  sent_when: offered on the Discord surface to managers and admins.
  used_by:
    - file: discord/gary/tools.rs
      function: tool_table
    - file: discord/gary/tools.rs
      function: dispatch_mutating
  reasoning:
    - >-
      A profile code is how modded groups already agree a modpack, so this is
      the fast path into a server's mod list — the description leads with that
      rather than with the mechanism, because the alternative it replaces is a
      friend reading twenty mod names out one at a time.
    - >-
      Says the list is replaced, not added to. Someone who expects a merge and
      gets a replacement loses the mods they had, and the mod list is rebuilt
      from this file at every start, so there is no partial state to notice
      afterwards.
    - >-
      Names the two things that are dropped on purpose — the mods the exporting
      player had switched off, and the ones the server does not run — because
      the result reports both and the model should be ready to explain them
      rather than treat them as errors.
    - >-
      Points at the restart, because writing the file changes nothing until one
      happens and a friend told "done" would go looking for mods that are not
      loaded yet.
---
Adopt a modpack from an r2modman profile code. Fetches what the code names, works out which of those mods belong on this server and which belong on the players' machines, and rewrites the server's mod list to match. This **replaces** the list — anything on it that the profile doesn't name is removed, so check what's there first if they meant to add to it rather than swap it. Mods the exporting player had switched off are left out, so is the mod loader the server image already provides, and so is anything this game's registry has no record of; the result names all three. Restart the server afterwards to apply it — and like any config change, a list that stops the server is rolled back automatically.
