---
id: StartServer
type: tool
params_from: NameParams
annotations:
  sent_when: offered on the Discord surface to managers and admins
  used_by:
    - file: src/discord/gary/tools.rs
      function: dispatch_tool
  reasoning:
    - shares NameParams so it stays in lockstep with the other lifecycle tools
---
Start a stopped server and report its address once it's ready.
