---
id: ReadFile
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
      Reads a config or text file so the model can see a setting's current value
      before editing. Shares PathParams with browse_files and restore_file.
---
Read a config or text file from a running server's data directory.
