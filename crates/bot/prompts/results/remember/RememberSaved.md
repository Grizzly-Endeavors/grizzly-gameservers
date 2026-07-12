---
id: RememberSaved
type: prompt
annotations:
  sent_when: >-
    tool result when remember successfully persists a fact to long-term memory
  used_by:
    - file: discord/gary/tools.rs
      function: exec_remember
  variables:
    scope:
      source: the normalized scope the fact was filed under
      contents: the scope name (a game id or 'general')
    id:
      source: the persisted fact's row id
      contents: the saved fact's id, as a number (used later by forget)
  reasoning:
    - >-
      Confirms the fact stuck and reports the id so the model (and later forget)
      can reference it. Both scope and id are data entering through variables.
---
saved that under {{scope}} (fact #{{id}}); I'll have it next time
