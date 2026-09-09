---
id: NegativeOffset
type: prompt
annotations:
  sent_when: >-
    tool result when read_file is called with a negative offset — the v1 tool
    schema carries offset as a signed integer, so a negative is possible on the
    wire and refused with an actionable message
  used_by:
    - file: discord/gary/tools.rs
      function: narrow_offset
  reasoning:
    - >-
      Its own message rather than sharing NegativeLineCount: the fix differs
      (omit it to start at the beginning, rather than pass a positive count),
      and a refusal that names the wrong argument sends the model editing the
      wrong part of its call.
---
the offset can't be negative — pass a positive byte offset, or omit it to read from the start of the file
