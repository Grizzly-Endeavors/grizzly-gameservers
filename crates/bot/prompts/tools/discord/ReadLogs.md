---
id: ReadLogs
type: tool
tool_schema:
  name:
    type: string
    description: Exact server name, as shown by `list_servers`.
  lines:
    type: integer
    description: >-
      How many trailing lines to return. Defaults to a recent window when
      omitted.
    optional: true
annotations:
  sent_when: offered on the Discord surface to managers and admins.
  used_by:
    - file: discord/gary/tools.rs
      function: manager_tools
    - file: discord/gary/tools.rs
      function: dispatch_mutating
  reasoning:
    - >-
      Its own shape rather than a NameParams tool because it carries the
      optional lines count. The description frames logs as the first place to
      look when something is wrong or to confirm a change took effect.
    - >-
      lines is a plain integer on the wire (i64); dispatch narrows it to the
      unsigned window the reader wants and refuses a negative value with a
      message the model can act on. Keeping the wire type wide keeps the schema
      inside the v1 vocabulary while the narrowing stays explicit in Rust.
---
Read the most recent output from a running server — the first place to look when something is wrong or to confirm a change took effect.
