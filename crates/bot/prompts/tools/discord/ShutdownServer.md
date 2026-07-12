---
id: ShutdownServer
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
      Frees the slot but keeps the world, so the description promises it can
      start later — distinguishing it from destroy (permanent) and stop (kept
      warm) so the model reaches for the right level of teardown.
---
Fully shut a server down to free its slot, keeping the world so it can start later.
