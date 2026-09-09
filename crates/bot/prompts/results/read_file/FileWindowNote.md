---
id: FileWindowNote
type: prompt
annotations:
  sent_when: >-
    appended to a read_file result whose window started partway into the file
  used_by:
    - file: discord/gary/tools.rs
      function: format_file
  variables:
    offset:
      source: the byte offset the returned window actually starts at
      contents: the offset as a number
  reasoning:
    - >-
      A window that does not start at zero has to say so, or the model reads a
      fragment as the whole file and concludes a setting is missing when it is
      simply above the window.
    - >-
      Reports where the window *actually* began rather than what was asked for:
      an offset landing inside a multi-byte character resumes at the next
      boundary, and a model paging by adding lengths needs the real number.
---
(starting at byte {{offset}})
