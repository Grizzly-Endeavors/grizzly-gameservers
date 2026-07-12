---
id: WriteFile
type: tool
tool_schema:
  name:
    type: string
    description: Exact server name, as shown by `list_servers`.
  path:
    type: string
    description: >-
      Path within the server's data directory to overwrite. The previous version
      is saved first so `restore_file` can undo the change.
  content:
    type: string
    description: The full new contents of the file.
annotations:
  sent_when: offered on the Discord surface to managers and admins.
  used_by:
    - file: discord/gary/tools.rs
      function: manager_tools
    - file: discord/gary/tools.rs
      function: dispatch_mutating
  reasoning:
    - >-
      The wholesale rewrite — for creating a file or replacing one entirely. The
      body deliberately points back at edit_file for single-setting changes so
      the model reserves this for when it really means to overwrite.
---
Overwrite a config file in a running server's data directory with entirely new contents — use this to create a file or rewrite one wholesale; prefer edit_file to change one setting. Saves the previous version first. The change takes effect on the next restart — restart and read the logs to confirm it's healthy.
