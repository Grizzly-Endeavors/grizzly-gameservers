---
id: ServerListEmpty
type: prompt
annotations:
  sent_when: >-
    tool result when a server listing is requested and no game servers are
    active — sent on both the Discord and in-game surfaces in place of the list
  used_by:
    - file: discord/gary/tools.rs
      function: format_server_list
  reasoning:
    - >-
      The model-facing empty-list copy (distinct from the human-facing embed
      wording in render.rs); worded so Gary relays "nothing running" rather than
      an empty blob. Shared verbatim across surfaces per ADR-008.
---
no game servers are running right now
