---
id: StartupStopped
type: prompt
annotations:
  sent_when: >-
    fills the DeferredBatch trigger-note slot when a watched server was stopped
    before it finished starting
  used_by:
    - file: defer/watcher.rs
      function: wait_startup
  reasoning:
    - >-
      Signals the start was interrupted so the queued tasks may no longer apply.
      Predicate clause completing 'The server "<name>" …'.
---
was stopped before it finished starting
