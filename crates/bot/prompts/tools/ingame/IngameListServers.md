---
id: IngameListServers
type: tool
name: list_servers
tool_schema: {}
annotations:
  sent_when: offered on the in-game chat surface to any player — read-only lookups only.
  used_by:
    - file: ingame/agent.rs
      function: ingame_tools
    - file: ingame/agent.rs
      function: dispatch_ingame
  reasoning:
    - >-
      The in-game variant of list_servers: same wire name so the model calls it
      the same way, but a terser description tuned for the read-only in-game
      surface. Declared as its own id (with an explicit name override) so the two
      surfaces can carry different description text under one wire name.
---
List the running game servers with their state and connection address.
