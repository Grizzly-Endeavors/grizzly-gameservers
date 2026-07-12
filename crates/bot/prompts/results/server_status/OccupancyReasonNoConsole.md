---
id: OccupancyReasonNoConsole
type: prompt
annotations:
  sent_when: >-
    the reason clause of an unknown-occupancy status when the console was reached
    but didn't answer the occupancy query
  used_by:
    - file: discord/gary/tools.rs
      function: occupancy_line
  reasoning:
    - >-
      Covers the "asked but got nothing back" case (the console fallback for any
      non-ready, non-empty outcome), so the model treats it as a transient miss
      rather than an empty server.
---
the console didn't answer
