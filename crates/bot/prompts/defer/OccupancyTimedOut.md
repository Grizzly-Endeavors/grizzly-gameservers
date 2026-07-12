---
id: OccupancyTimedOut
type: prompt
annotations:
  sent_when: >-
    fills the DeferredBatch trigger-note slot when an 'empty'/'idle' wait reached
    its ceiling without the server ever emptying
  used_by:
    - file: defer/watcher.rs
      function: wait_occupancy
  reasoning:
    - >-
      The occupancy wait gave up: the server never cleared within the ceiling, so
      the queued change couldn't be applied unattended. Predicate clause
      completing 'The server "<name>" …'.
---
still hasn't emptied out after a long wait
