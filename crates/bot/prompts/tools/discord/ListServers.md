---
id: ListServers
type: tool
name: list_servers
tool_schema: {}
annotations:
  sent_when: offered on the Discord surface to every caller — read-only, manager, and admin.
  used_by:
    - file: discord/gary/tools.rs
      function: available_tools
    - file: discord/gary/tools.rs
      function: dispatch
  reasoning:
    - >-
      The read-only entry point: it names every server in the caller's scope so
      the model can pick one to act on. Zero-argument by design.
    - >-
      The wire name is declared explicitly because the in-game surface offers a
      terser variant under the same name (IngameListServers); the all-explicit
      collision policy requires every colliding file to mark the shared name.
---
List every game server and its state and connection address.
