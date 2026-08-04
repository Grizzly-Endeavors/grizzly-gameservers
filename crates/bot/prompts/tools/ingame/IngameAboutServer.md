---
id: IngameAboutServer
type: tool
name: about_server
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
      The in-game variant of about_server: same wire name and the same shared
      NameParams shape as the Discord tool, with a terser description for the
      in-game surface. The explicit name override marks it as a deliberate
      variant, satisfying the all-explicit wire-name collision policy.
    - >-
      Deliberately narrower than the Discord description: the in-game surface is
      read-only, so backup history and crash counts are of no use to a player
      standing in the world asking who else is on.
---
Look up one server by its exact name: its current state, address, and who is online.
