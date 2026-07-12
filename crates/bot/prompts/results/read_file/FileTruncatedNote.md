---
id: FileTruncatedNote
type: prompt
annotations:
  sent_when: >-
    spliced into a read_file result (via FileContents' note slot) when the file
    was too large to return whole and was cut short
  used_by:
    - file: discord/gary/tools.rs
      function: format_file
  reasoning:
    - >-
      Tells the model the file view is partial so it doesn't reason over a
      truncated file as if complete. Carries no leading space — the separator
      between the path and this parenthetical is owned by the assembling code.
---
(showing the first part; the file is larger and was truncated)
