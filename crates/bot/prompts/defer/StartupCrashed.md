---
id: StartupCrashed
type: prompt
annotations:
  sent_when: >-
    fills the DeferredBatch trigger-note slot when a watched (re)start comes up
    unhealthy — the server crashed while starting
  used_by:
    - file: defer/watcher.rs
      function: wait_startup
  reasoning:
    - >-
      Names the likely cause (a bad config change) so Gary investigates the crash
      before running the queued tasks — DeferredBatch tells him not to run them
      blindly on an unhealthy server. Predicate clause completing 'The server
      "<name>" …'; keep the "usual cause" hint, it steers the diagnosis.
---
came up unhealthy — it crashed while starting (a bad config change is the usual cause)
