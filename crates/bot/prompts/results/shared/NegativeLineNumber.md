---
id: NegativeLineNumber
type: prompt
annotations:
  sent_when: >-
    tool result when read_file is called with a negative start_line
  used_by:
    - file: discord/gary/tools.rs
      function: narrow_window
  reasoning:
    - >-
      The tool vocabulary has no unsigned integer, so a negative line number
      reaches the dispatcher and has to be refused there. Phrased as an
      instruction the model can act on rather than a validation error, matching
      how the other narrowing refusals read.
---
a line number can't be negative — pass a positive line to start from, or omit it to read from the top of the file
