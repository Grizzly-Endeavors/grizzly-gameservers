---
id: IngameGamesLine
type: prompt
annotations:
  sent_when: appended to the in-game system prompt for every in-game question
  used_by:
    - file: ingame/agent.rs
      function: build_ingame_system_prompt
  variables:
    games:
      source: >-
        the game catalog at the in-game call site in ingame/agent.rs
        (catalog.game_ids() joined with ", ").
      contents: >-
        comma-separated catalog game ids (e.g. "minecraft, valheim"), or the
        literal "(none)" when the catalog is empty — the empty-case fallback is
        applied in code before rendering.
  reasoning:
    - >-
      Distinct wording from Discord's DiscordGamesLine ("Games that can be
      launched:") — kept as its own file rather than shared, since the two
      surfaces phrase it differently. The list is computed data and enters
      through the variable; only the framing is prompt text.
---
Games that can be launched: {{games}}
