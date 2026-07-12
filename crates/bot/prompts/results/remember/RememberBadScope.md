---
id: RememberBadScope
type: prompt
annotations:
  sent_when: >-
    tool result when remember is given a scope that isn't a known game or
    'general'
  used_by:
    - file: discord/gary/tools.rs
      function: exec_remember
  variables:
    games:
      source: the catalog game ids, joined in exec_remember
      contents: comma-separated catalog game ids that are valid scopes (alongside 'general')
  reasoning:
    - >-
      Enumerates the valid scopes so the model refiles the fact under a real one
      rather than guessing. The game list is data; the 'general' option is fixed
      prose because it's always available regardless of the catalog.
---
I can only file that under a game or 'general'. Pick one of: {{games}}, general.
