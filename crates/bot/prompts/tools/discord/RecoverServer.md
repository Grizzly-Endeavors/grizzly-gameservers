---
id: RecoverServer
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
      The inverse of archive: recreates an archived server and restores its
      world. Constructive, so it runs without a confirmation. The description
      points the model at list_archives for the right name, since a recovered
      server isn't in the live listing the scope gate uses.
---
Bring an archived server back: recreate it and restore its world from the archive. Use the name shown by list_archives. Constructive, so it runs without a confirmation.
