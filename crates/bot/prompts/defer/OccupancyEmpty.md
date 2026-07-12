---
id: OccupancyEmpty
type: prompt
annotations:
  sent_when: >-
    fills the DeferredBatch trigger-note slot when an 'empty' wait fires — the
    server's player count first reached zero
  used_by:
    - file: defer/watcher.rs
      function: wait_occupancy
  reasoning:
    - >-
      The 'empty' condition's success note: the moment the last player left, so a
      restart-needing change can run without kicking anyone. Predicate clause
      completing 'The server "<name>" …'.
---
is now empty — no players are connected
