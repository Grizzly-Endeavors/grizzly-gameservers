---
id: RememberNeedFact
type: prompt
annotations:
  sent_when: >-
    tool result when remember is called with an empty note, so there's nothing to
    save
  used_by:
    - file: discord/gary/tools.rs
      function: exec_remember
  reasoning:
    - >-
      Asks the model for the actual fact in a short sentence rather than saving
      nothing, steering it to supply content instead of retrying an empty call.
---
I need something to remember — give me the fact in a short sentence.
