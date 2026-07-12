---
id: IngameServerStatus
type: tool
name: server_status
params_from: NameParams
annotations:
  sent_when: offered on the in-game chat surface to any player — read-only lookups only.
  used_by:
    - file: ingame/agent.rs
      function: ingame_tools
    - file: ingame/agent.rs
      function: dispatch_ingame
  reasoning:
    - >-
      The in-game variant of server_status: same wire name and the same shared
      NameParams shape as the Discord tool, with a terser description for the
      in-game surface. The explicit name override marks it as a deliberate
      variant, satisfying the all-explicit wire-name collision policy.
---
Look up one server's current state and address by its exact name.
