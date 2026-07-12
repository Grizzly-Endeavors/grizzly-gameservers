---
id: RememberMemoryOffline
type: prompt
annotations:
  sent_when: >-
    tool result when remember can't reach long-term memory storage, so the fact
    only survives the current conversation
  used_by:
    - file: discord/gary/tools.rs
      function: exec_remember
  reasoning:
    - >-
      Sets the model's expectation precisely — the fact holds for this
      conversation but won't persist — so Gary doesn't promise permanence it
      can't deliver, while still using the fact for now.
---
my long-term memory's offline right now, so I can't save that. It'll stick for the rest of this conversation but not beyond it.
