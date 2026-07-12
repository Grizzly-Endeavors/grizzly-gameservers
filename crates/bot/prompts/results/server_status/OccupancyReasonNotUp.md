---
id: OccupancyReasonNotUp
type: prompt
annotations:
  sent_when: >-
    the reason clause of an unknown-occupancy status when the server pod isn't
    fully up yet, so the count can't be read
  used_by:
    - file: discord/gary/tools.rs
      function: occupancy_line
  reasoning:
    - >-
      A transient reason (the server is still coming up), distinct from the
      game-can't-report case, so the model knows a retry after boot could work.
---
the server isn't fully up yet
