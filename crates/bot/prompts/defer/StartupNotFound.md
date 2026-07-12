---
id: StartupNotFound
type: prompt
annotations:
  sent_when: >-
    fills the DeferredBatch trigger-note slot when the watched server was removed
    before it came up
  used_by:
    - file: defer/watcher.rs
      function: wait_startup
  reasoning:
    - >-
      The server is gone, so the queued tasks can't run; Gary should say so rather
      than force them. Predicate clause completing 'The server "<name>" …'.
      Distinct from OccupancyNotFound only in the "before it came up" tail, which
      matches the startup context.
---
no longer exists — it was removed before it came up
