---
id: ColdStartUnknownGame
type: prompt
annotations:
  sent_when: >-
    tool result when start_server tries to boot a server whose game is no longer
    in the catalog
  used_by:
    - file: discord/gary/tools.rs
      function: exec_cold_start
  variables:
    server:
      source: the server being started
      contents: the server name
    game:
      source: the server's stored game id
      contents: the game id that's no longer in the catalog, quoted inline
  reasoning:
    - >-
      Explains why a known server won't start (its game was removed) using the
      present tense "runs", distinct from the recover path's "ran" — the server
      still exists, only its game reference is stale.
---
{{server}} runs '{{game}}', which isn't in the catalog anymore
