---
id: StartupNotManaged
type: prompt
annotations:
  sent_when: >-
    fills the DeferredBatch trigger-note slot when the watched target isn't a
    server Gary can manage, so its startup couldn't be watched
  used_by:
    - file: defer/watcher.rs
      function: wait_startup
  reasoning:
    - >-
      The watch couldn't proceed because the target is out of scope; Gary reports
      that rather than pretending the start was observed. Predicate clause
      completing 'The server "<name>" …'. Distinct from OccupancyNotManaged only
      in the "watch it start" tail.
---
isn't a server I can manage, so I couldn't watch it start
