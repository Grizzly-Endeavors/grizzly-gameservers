---
id: PathParams
type: params
tool_schema:
  name:
    type: string
    description: Exact server name, as shown by `list_servers`.
  path:
    type: string
    description: >-
      Path within the server's data directory, e.g. `server.properties` or
      `logs/latest.log`. Use `""` for the top of the data directory. Must stay
      inside the data directory — absolute paths and `..` are refused.
annotations:
  used_by:
    - file: discord/gary/tools.rs
      function: dispatch_mutating
  reasoning:
    - >-
      browse_files, read_file, and restore_file all take the same
      server-name-plus-path pair with identical rules, so they share this one
      shape. Tuning the path description (the sandbox rules the model must
      respect) in one place keeps those three tools from drifting apart.
---
