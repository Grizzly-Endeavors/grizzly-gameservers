---
id: CreateUnknownGame
type: prompt
annotations:
  sent_when: >-
    tool result when create_server is asked for a game that isn't in the catalog
  used_by:
    - file: discord/gary/tools.rs
      function: exec_create
  variables:
    game:
      source: the game id the model asked to launch
      contents: the unrecognized game id, quoted inline
    games:
      source: the catalog game ids, joined by game_ids in exec_create
      contents: comma-separated catalog game ids the model can actually launch
  reasoning:
    - >-
      Names the bad game and lists the real options so the model corrects to a
      launchable game rather than reinventing one. The available list is data
      entering through the variable.
---
'{{game}}' isn't a game I can launch. Available games: {{games}}.
