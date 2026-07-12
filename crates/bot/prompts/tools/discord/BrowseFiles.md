---
id: BrowseFiles
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
      The entry point for the file-tuning loop: the description tells the model
      to start here with the empty path and descend, so it discovers which file
      holds a setting instead of guessing a path.
---
List the files and folders in a running server's data directory. Use "" for the top level, then descend. Start here to find which file holds a setting.
