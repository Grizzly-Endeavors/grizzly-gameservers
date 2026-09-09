---
id: FileContinuation
type: prompt
annotations:
  sent_when: >-
    appended to a read_file result that was cut short, naming the offset that
    continues it
  used_by:
    - file: discord/gary/tools.rs
      function: format_file
  variables:
    offset:
      source: >-
        the offset that resumes where this window stopped, computed in
        format_file as the window's start plus the bytes returned
      contents: the continuation offset as a number
  reasoning:
    - >-
      Turns "this was cut short" into something actionable in the same breath.
      Without it a model that sees truncation has to guess the arithmetic, and
      guessing wrong silently re-reads the same window forever.
---
(read on from byte {{offset}})
