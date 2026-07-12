---
id: StartupReady
type: prompt
annotations:
  sent_when: >-
    fills the DeferredBatch trigger-note slot when a watched (re)start finishes
    healthy and is accepting players
  used_by:
    - file: defer/watcher.rs
      function: wait_startup
  reasoning:
    - >-
      The happy-path startup outcome. Phrased as a predicate that completes 'The
      server "<name>" …' inside DeferredBatch, so keep it a bare clause with no
      leading subject or trailing period.
---
has finished starting up and is accepting players
