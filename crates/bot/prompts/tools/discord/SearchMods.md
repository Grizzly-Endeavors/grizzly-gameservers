---
id: SearchMods
type: tool
tool_schema:
  game:
    type: string
    description: >-
      Which game's registry to search, as a catalog id — `minecraft`,
      `valheim`. Take it from the server you are working on; mods never cross
      between games, so a search without the right game answers a different
      question.
  query:
    type: string
    description: >-
      What to search for, in plain words — a name if the user gave one
      ("Terralith", "Jotunn"), otherwise what they want it to do ("better world
      generation", "minimap").
  side:
    type: enum
    description: >-
      Only mods that have to be installed in this place: `server` (the server
      needs it), `client` (players' machines need it), `both` (everyone needs
      it, including every player). Omit unless the user's ask turns on it — most
      searches want to see all of them and weigh the client cost per result.
    optional: true
    values:
      - server
      - client
      - both
  kind:
    type: enum
    description: >-
      Minecraft only. Narrow to one kind: `datapack` (works on a vanilla server,
      nothing for players to install) or `mod` (needs a loader such as Fabric on
      the server). Omit to search both.
    values:
      - mod
      - datapack
    optional: true
  loader:
    type: enum
    description: >-
      Minecraft only. Only projects supporting this loader. Use the loader the
      server already runs, or `fabric` when picking one for a server that has
      none yet.
    optional: true
    values:
      - fabric
      - forge
      - neoforge
      - quilt
  game_version:
    type: string
    description: >-
      Minecraft only. Only projects supporting this exact Minecraft version,
      e.g. `26.2`. Use the version the server actually runs — mod compatibility
      is scoped to it.
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
      The discovery half of mod management on every game that has a registry. It
      exists because a name recalled from training data is often wrong or
      renamed, and a wrong entry in a server's mod list stops its next launch —
      so the description steers toward searching first rather than typing a
      remembered name straight into a config.
    - >-
      game is required rather than inferred because each game has its own
      registry and the identifiers are not interchangeable — a Minecraft slug
      means nothing to a Valheim search, and answering the wrong game's question
      confidently is worse than asking. It also decides which of the filters
      below apply, which is why they name their game rather than reading as
      universal.
    - >-
      kind and loader are closed enums so the model can't invent a project type
      or a loader the registry doesn't index. kind omitted searches both, which
      matters because a project can ship as each — asking for one kind is a
      narrowing, not a classification of the project.
    - >-
      The kind descriptions carry the client-install consequence, since that is
      the single most important thing to get right when recommending something
      to a non-technical friend: a datapack costs the players nothing, a mod may
      require every one of them to install it. side is the same question asked
      as a filter, and it is discouraged by default because a result the model
      never sees is a tradeoff it can't tell the user about.
    - >-
      Points at game_guide rather than restating each game's install workflow:
      the workflows differ per registry and are loaded on demand, so duplicating
      them here would put two copies of the same instructions in front of the
      model on every manager turn.
---
Search a game's mod registry. Use this before naming any mod — identifiers change and guessing one from memory breaks the server on its next restart. Pass the `game` whose registry you want; each game has its own, and the results only mean anything for that game. Returns a handful of matches with each mod's exact identifier (the one a server's config needs), what it does, how popular it is, and where it has to be installed — the server, players' machines, or both. Call `game_guide` for the same game to learn how to install what you find.
