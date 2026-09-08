---
id: ModGameUnknown
type: prompt
annotations:
  sent_when: >-
    tool result when search_mods or mod_info is called with a game that isn't in
    the catalog at all
  used_by:
    - file: discord/gary/tools.rs
      function: mod_game_unknown
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
      A wrong game is the one argument mistake with no signal of its own: the
      registry call would either fail or, worse, succeed against a different
      community and answer confidently about the wrong game. Naming the catalog
      turns that into a one-step correction.
    - >-
      Distinguished from ModsNotSupported, which is a real game that takes no
      mods — that answer is final, this one is a typo to fix.
---
{{game}} isn't a game this bot knows. The games are: {{games}}. Check which game the server actually runs (about_server reports it) and try again with that id.
