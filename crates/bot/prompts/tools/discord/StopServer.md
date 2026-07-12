---
id: StopServer
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
      Non-destructive pause — the world is saved and the pod kept warm, so the
      description contrasts it with shutdown_server (which frees the slot) to
      steer the model to the lighter option when someone just wants a break.
---
Pause a running server in place (world saved, kept warm for a fast start).
