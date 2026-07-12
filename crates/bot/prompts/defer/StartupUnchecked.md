---
id: StartupUnchecked
type: prompt
annotations:
  sent_when: >-
    fills the DeferredBatch trigger-note slot when the cluster couldn't be queried
    while watching a (re)start, so the outcome is unknown
  used_by:
    - file: defer/watcher.rs
      function: wait_startup
  reasoning:
    - >-
      A cluster-query failure, not a server verdict: the startup outcome is
      genuinely unknown, so Gary shouldn't assume success or crash. Predicate
      clause completing 'The server "<name>" …'.
---
couldn't be checked — the cluster didn't answer while I watched it start
