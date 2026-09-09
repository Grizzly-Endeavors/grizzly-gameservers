---
id: ReadFile
type: tool
tool_schema:
  name:
    type: string
    description: Exact server name, as shown by `list_servers`.
  path:
    type: string
    description: Path to the file, relative to the server's data directory.
  offset:
    type: integer
    description: >-
      Byte offset to start reading from. Omit for the start of the file; when a
      read comes back marked as cut short, the result says which offset
      continues it.
    optional: true
annotations:
  sent_when: offered on the Discord surface to managers and admins.
  used_by:
    - file: discord/gary/tools.rs
      function: tool_table
    - file: discord/gary/tools.rs
      function: dispatch_files
  reasoning:
    - >-
      Reads a config or text file so the model can see a setting's current value
      before editing.
    - >-
      Its own shape rather than the shared PathParams because of the offset. A
      read is capped, and the cap keeps the *front* of the file, so without an
      offset the end of anything long was unreachable by this tool at all — the
      model could see that a file was cut short and had no way to act on it.
    - >-
      offset is a plain integer on the wire (i64); dispatch narrows it to the
      unsigned window the reader wants and refuses a negative value with a
      message the model can act on, matching how read_logs handles its count.
---
Read a config or text file from a running server's data directory.
