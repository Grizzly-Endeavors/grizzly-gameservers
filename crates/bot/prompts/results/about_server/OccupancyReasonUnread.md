---
id: OccupancyReasonUnread
type: prompt
annotations:
  sent_when: >-
    the reason clause of an unknown-occupancy status when the occupancy lookup
    itself errored
  used_by:
    - file: discord/gary/tools.rs
      function: occupancy_line
  reasoning:
    - >-
      The catch-all read-failure reason (the lookup threw), kept vague on purpose
      because the underlying error is logged for developers, not shown to the
      model — Gary just needs to know the count is unavailable.
---
the count couldn't be read
