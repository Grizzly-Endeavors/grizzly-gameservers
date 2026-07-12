---
id: ServerNotFound
type: prompt
annotations:
  sent_when: >-
    tool result when a tool targets a server name that isn't in the caller's
    scope — returned to the model so it re-lists rather than reporting a dead end
  used_by:
    - file: discord/gary/tools.rs
      function: no_such
  variables:
    server:
      source: the server name the model passed to the tool
      contents: the unresolved server name, quoted inline
  reasoning:
    - >-
      A tool result the model reads, so it carries the steer to re-list (check
      list_servers) rather than just reporting the server missing — keeps Gary
      recovering with the current names instead of insisting on a stale one.
---
there's no server named {{server}} — check list_servers for the current names
