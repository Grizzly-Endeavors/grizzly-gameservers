---
id: EditFile
type: tool
tool_schema:
  path:
    type: string
    description: path to the file on the server's data volume
  mode:
    type: enum
    description: whether to replace the whole file or append to it
    values:
      - replace
      - append
  count:
    type: integer
    description: number of lines to write; omit to write the whole payload
    optional: true
annotations:
  sent_when: offered on the Discord surface to managers and admins
  used_by:
    - file: discord/gary/tools.rs
      function: dispatch_tool
  reasoning:
    - the enum keeps edit modes closed so the model can't invent a third mode
---
Edit a configuration file on the server, then verify and roll back on failure.
