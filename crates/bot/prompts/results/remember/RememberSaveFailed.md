---
id: RememberSaveFailed
type: prompt
annotations:
  sent_when: >-
    tool result when remember errors while writing to memory (an unexpected
    failure, not a clean "offline")
  used_by:
    - file: discord/gary/tools.rs
      function: exec_remember
  reasoning:
    - >-
      Tells the model the save genuinely failed (nothing stuck) so it doesn't
      claim success; the underlying error is logged for developers, not shown
      here.
---
something went wrong saving that — it didn't stick.
