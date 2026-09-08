---
id: GameGuideUnknownGame
type: prompt
annotations:
  sent_when: >-
    the whole game_guide result when the game argument names nothing in the
    catalog
  used_by:
    - file: discord/gary/guide.rs
      function: render_guide
  variables:
    game:
      source: the game argument the model passed to the tool
      contents: the unrecognised game id, quoted inline
    games:
      source: every game id in the catalog, comma-joined
      contents: >-
        the catalog's game ids, comma-separated, or the literal "(none)" when
        the catalog is empty — the fallback is applied in code before rendering
  reasoning:
    - >-
      A typo to correct rather than an answer, so it names the catalog and the
      one tool that reports which game a given server actually runs. Without
      that, the correction is a guess.
---
{{game}} isn't a game this bot knows, so there is no guide for it. The games are: {{games}}. If you're working on a specific server, about_server reports which game it runs — ask again with that id.
