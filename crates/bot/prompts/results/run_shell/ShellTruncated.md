---
id: ShellTruncated
type: prompt
annotations:
  sent_when: >-
    appended to a run_shell result whose output exceeded the capture cap
  used_by:
    - file: discord/gary/tools.rs
      function: format_shell
  reasoning:
    - >-
      Says which end survived, because the retained bytes are the first ones: a
      model that assumed it was holding the tail would draw the wrong conclusion
      from a truncated log dump. Suggesting the narrowing is the point — this is
      the moment the model should reach for grep rather than cat.
---
(only the first part of the output is shown — narrow the command, or pipe it through grep, to see the rest.)
