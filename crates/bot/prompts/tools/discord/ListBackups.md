---
id: ListBackups
type: tool
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
      Read-only view of a server's restore points so the model can reason about
      what restore_server could roll back to before proposing anything risky.
---
List a server's saved world backups (newest first), so you can see what points it could be restored to.
