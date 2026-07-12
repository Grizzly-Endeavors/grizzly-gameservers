---
id: RecoverUnknownGame
type: prompt
annotations:
  sent_when: >-
    tool result when recover_server can't recreate an archive because the game it
    ran is no longer in the catalog
  used_by:
    - file: discord/gary/tools.rs
      function: format_recover
  variables:
    name:
      source: the archived server's name
      contents: the archived server name
    game:
      source: the archived server's stored game id
      contents: the game id that's no longer in the catalog, quoted inline
  reasoning:
    - >-
      Explains why the archive can't be recovered (its game was removed) using
      past tense "ran" — the server no longer exists, distinct from the cold-start
      "runs" wording for a live server whose game went missing.
---
{{name}} ran '{{game}}', which isn't in the catalog anymore
