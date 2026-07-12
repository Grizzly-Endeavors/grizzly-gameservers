---
id: RestoreFile
type: tool
params_from: PathParams
annotations:
  sent_when: offered on the Discord surface to managers and admins.
  used_by:
    - file: discord/gary/tools.rs
      function: manager_tools
    - file: discord/gary/tools.rs
      function: dispatch_mutating
  reasoning:
    - >-
      The undo for edit_file/write_file — restores the version saved before the
      last write. The description reminds the model to restart afterward, since
      the restore only takes effect on the next boot. Shares PathParams.
---
Undo the last write to a file by restoring the version saved before it. Restart afterward to apply.
