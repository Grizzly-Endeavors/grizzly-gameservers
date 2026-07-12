---
id: OccupancyNotFound
type: prompt
annotations:
  sent_when: >-
    fills the DeferredBatch trigger-note slot when the watched server was removed
    before it went empty
  used_by:
    - file: defer/watcher.rs
      function: wait_occupancy
  reasoning:
    - >-
      The server is gone, so the queued tasks can't run; Gary should say so rather
      than force them. Predicate clause completing 'The server "<name>" …'.
      Distinct from StartupNotFound only in the "before it went empty" tail.
---
no longer exists — it was removed before it went empty
