---
id: OccupancyNotManaged
type: prompt
annotations:
  sent_when: >-
    fills the DeferredBatch trigger-note slot when the watched target isn't a
    server Gary can manage, so its occupancy couldn't be watched
  used_by:
    - file: defer/watcher.rs
      function: wait_occupancy
  reasoning:
    - >-
      The watch couldn't proceed because the target is out of scope; Gary reports
      that rather than pretending it emptied. Predicate clause completing 'The
      server "<name>" …'. Distinct from StartupNotManaged only in the "watch it
      clear" tail.
---
isn't a server I can manage, so I couldn't watch it clear
