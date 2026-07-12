---
id: StartupTimedOut
type: prompt
annotations:
  sent_when: >-
    fills the DeferredBatch trigger-note slot when a watched (re)start never
    became ready within the startup ceiling
  used_by:
    - file: defer/watcher.rs
      function: wait_startup
  reasoning:
    - >-
      Frames a stuck-looking startup (not a confirmed crash) so Gary treats the
      server as not-yet-healthy and looks into it. Predicate clause completing
      'The server "<name>" …'.
---
still isn't up after a long wait, so its startup looks stuck
