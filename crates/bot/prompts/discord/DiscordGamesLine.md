---
id: DiscordGamesLine
type: prompt
annotations:
  sent_when: appended to every Discord system prompt, on every access tier
  used_by:
    - file: discord/gary/mod.rs
      function: build_system_prompt
  variables:
    games:
      source: >-
        the game catalog, joined by game_catalog_list in discord/gary/mod.rs
        (catalog.game_ids()).
      contents: >-
        comma-separated catalog game ids (e.g. "minecraft, valheim"), or the
        literal "(none)" when the catalog is empty — the empty-case fallback is
        applied in code before rendering.
  reasoning:
    - >-
      Tells Gary which games he can actually launch, so he doesn't offer or
      invent one that isn't in the catalog. The list itself is computed data and
      enters through the variable; only the "Available games to launch:" framing
      is prompt text.
---
Available games to launch: {{games}}
