---
id: OccupancyIdle
type: prompt
annotations:
  sent_when: >-
    fills the DeferredBatch trigger-note slot when an 'idle' wait fires — the
    server stayed empty for the full grace window
  used_by:
    - file: defer/watcher.rs
      function: wait_occupancy
  reasoning:
    - >-
      The 'idle' condition's success note: distinct from OccupancyEmpty because
      idle waits out a grace window, so this says "for a while now" rather than
      "just now". Predicate clause completing 'The server "<name>" …'.
---
has been empty for a while now
