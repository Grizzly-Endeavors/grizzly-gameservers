---
id: OccupancyReasonNoCount
type: prompt
annotations:
  sent_when: >-
    the reason clause of an unknown-occupancy status when the game doesn't expose
    a live player count at all
  used_by:
    - file: discord/gary/tools.rs
      function: occupancy_line
  reasoning:
    - >-
      Marks this as a permanent limitation of the game (not a transient read
      failure), so the model doesn't keep retrying occupancy or promise to watch
      for an empty server it can't measure.
---
this game doesn't report a live player count
