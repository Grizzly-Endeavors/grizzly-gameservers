---
id: RestoreServerTimedOut
type: prompt
annotations:
  sent_when: >-
    tool result when restore_server restores the world but the server hasn't
    finished booting within the wait window
  used_by:
    - file: discord/gary/tools.rs
      function: format_restore_outcome
  variables:
    server:
      source: the server that was restored
      contents: the server name
  reasoning:
    - >-
      Reports the restore succeeded and boot is just slow (playable in a minute),
      so the model sets a short-wait expectation rather than implying failure.
---
restored the world onto {{server}} — it'll be playable again in a minute
