---
id: BackupServer
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
      The safety net before a risky change: the description stresses it's
      non-destructive and pairs it with restore_server, so the model takes a
      backup first rather than editing blind.
---
Save a durable backup of a running server's world right now. Non-destructive — the server keeps running. Use before a risky change so restore_server can roll it back.
