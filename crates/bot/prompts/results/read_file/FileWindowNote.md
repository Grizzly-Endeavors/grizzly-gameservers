---
id: FileWindowNote
type: prompt
annotations:
  sent_when: >-
    appended to a read_file result whenever the window shown is not the whole
    file
  used_by:
    - file: discord/gary/tools.rs
      function: format_file
  variables:
    first:
      source: the 1-based line the returned window starts at
      contents: a line number
    last:
      source: the 1-based line the returned window ends at
      contents: a line number
    total:
      source: the file's total line count, as counted in the pod
      contents: a line number
  reasoning:
    - >-
      Says which slice of the file is in view and how big the file is, so a
      partial read can't be mistaken for the whole thing. Without the total the
      model has no way to tell a complete small file from the head of a large
      one, which is the mistake that leads to confident answers about settings
      that were never read.
    - >-
      Phrased as a range rather than an offset because the next read is
      addressed in lines: the number to ask for next is visible in the note
      itself, with nothing to compute.
---
(lines {{first}}–{{last}} of {{total}})
