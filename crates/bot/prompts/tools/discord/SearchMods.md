---
id: SearchMods
type: tool
tool_schema:
  query:
    type: string
    description: >-
      What to search for, in plain words — a name if the user gave one
      ("Terralith"), otherwise what they want it to do ("better world
      generation", "minimap").
  kind:
    type: enum
    description: >-
      Narrow to one kind: `datapack` (works on a vanilla server, nothing for
      players to install) or `mod` (needs a loader such as Fabric on the
      server). Omit to search both.
    values:
      - mod
      - datapack
    optional: true
  loader:
    type: enum
    description: >-
      Only projects supporting this loader. Use the loader the server already
      runs, or `fabric` when picking one for a server that has none yet.
    optional: true
    values:
      - fabric
      - forge
      - neoforge
      - quilt
  game_version:
    type: string
    description: >-
      Only projects supporting this exact Minecraft version, e.g. `26.2`. Use
      the version the server actually runs — mod compatibility is scoped to it.
    optional: true
annotations:
  sent_when: offered on the Discord surface to managers and admins.
  used_by:
    - file: discord/gary/tools.rs
      function: tool_table
    - file: discord/gary/tools.rs
      function: dispatch
  reasoning:
    - >-
      The discovery half of Minecraft mod management. It exists because a slug
      recalled from training data is often wrong or renamed, and a wrong slug
      hard-fails the server's next boot — so the description steers toward
      searching first rather than typing a remembered name straight into the
      config.
    - >-
      kind and loader are closed enums so the model can't invent a project type
      or a loader the registry doesn't index. kind is deliberately not a
      passthrough of the registry's own project type, which has no datapack
      value; the mapping to category filters is code's business, and the model
      is offered the distinction a user actually means.
    - >-
      The kind descriptions carry the client-install consequence, since that is
      the single most important thing to get right when recommending something
      to a non-technical friend: a datapack costs the players nothing, a mod may
      require every one of them to install it.
---
Search Modrinth for Minecraft mods and datapacks. Use this before naming any mod or datapack — slugs change and guessing one from memory breaks the server on its next restart. Returns a handful of matches with each project's exact slug (the identifier the server config needs), what it does, how many downloads it has, whether it's needed on the server, on players' machines, or both, and which loaders and Minecraft versions it supports.
