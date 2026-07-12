---
id: RestoreServer
type: tool
params_from: NameParams
annotations:
  sent_when: offered on the Discord surface to admins only.
  used_by:
    - file: discord/gary/tools.rs
      function: admin_only_tools
    - file: discord/gary/tools.rs
      function: dispatch_mutating
  reasoning:
    - >-
      Rolls the world back to its most recent backup, overwriting the current
      one. The description promises a safety backup of the current world is taken
      first, and that the user must approve the posted confirmation before the
      overwrite — this destroys current progress if misused.
---
Roll a server back to its most recent backup, replacing the current world (a safety backup of the current world is taken first). Posts a confirmation the user must approve before the world is overwritten.
