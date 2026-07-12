---
id: ServerStatus
type: tool
name: server_status
params_from: NameParams
annotations:
  sent_when: offered on the Discord surface to every caller — read-only, manager, and admin.
  used_by:
    - file: discord/gary/tools.rs
      function: available_tools
    - file: discord/gary/tools.rs
      function: dispatch
  reasoning:
    - >-
      The single-server lookup. Shares NameParams with the lifecycle tools so
      the server-name argument means the same thing everywhere.
    - >-
      The wire name is declared explicitly because the in-game surface offers a
      terser variant under the same name (IngameServerStatus); the all-explicit
      collision policy requires every colliding file to mark the shared name.
---
Look up one server's current state and address by name.
