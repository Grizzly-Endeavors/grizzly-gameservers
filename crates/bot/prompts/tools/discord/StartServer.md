---
id: StartServer
type: tool
params_from: NameParams
annotations:
  sent_when: offered on the Discord surface to managers and admins.
  used_by:
    - file: discord/gary/tools.rs
      function: manager_tools
    - file: discord/gary/tools.rs
      function: dispatch_mutating
  reasoning:
    - >-
      One tool covers both resuming a paused server and cold-starting a
      shut-down one, so the model doesn't have to know which state it's in — the
      description names both cases so it picks this for either.
---
Start a server: resume a paused one or bring a stopped one back up.
