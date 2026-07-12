---
id: NegativeLineCount
type: prompt
annotations:
  sent_when: >-
    tool result when read_logs is called with a negative line count — the v1
    tool schema carries lines as a signed integer, so a negative is possible on
    the wire and refused with an actionable message
  used_by:
    - file: discord/gary/tools.rs
      function: narrow_lines
  reasoning:
    - >-
      Refuses the bad argument with a fix the model can act on (pass a positive
      count or omit it) rather than erroring the loop, matching the recover-not-
      dead-end convention of the other soft-failure results.
---
the line count can't be negative — pass a positive number of lines, or omit it for a recent window
