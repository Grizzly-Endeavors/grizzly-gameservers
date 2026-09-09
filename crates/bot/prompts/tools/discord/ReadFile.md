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
  start_line:
    type: integer
    description: >-
      1-based line to start reading from. Omit to start at the top of the file.
    optional: true
  line_count:
    type: integer
    description: >-
      How many lines to return. Omit for a default window; the result says how
      many lines the file has in total, so you can tell what you haven't seen.
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
      Its own shape rather than the shared PathParams because of the window. A
      read is bounded, so a long file needs a way to ask for the part that
      matters; without one the end of anything long is unreachable by this tool.
    - >-
      The window is addressed in lines rather than bytes because the model can
      derive a line from a grep hit or from a window it has already read, and can
      derive a byte offset from neither. Lines also fall on entry boundaries in
      the config formats games actually use.
    - >-
      Both are plain integers on the wire (i64); dispatch narrows them to the
      unsigned window the reader wants and refuses a negative value with a
      message the model can act on, matching how read_logs handles its count.
---
Read a file from a running server's data directory. Use `run_shell` to find the content you're looking for first and then read the surrounding sections. Reading the whole file front to back is rarely the fastest way to find what you're looking for.
