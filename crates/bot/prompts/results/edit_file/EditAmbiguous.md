---
id: EditAmbiguous
type: prompt
annotations:
  sent_when: >-
    tool result when edit_file's old_text matches more than once, so the target
    is ambiguous and no change was made
  used_by:
    - file: discord/gary/tools.rs
      function: format_edit
  variables:
    count:
      source: the number of times old_text matched
      contents: the match count, as a number
    path:
      source: the file path the edit targeted
      contents: the file path
  reasoning:
    - >-
      Explains why the edit was refused (multiple matches) and how to fix it —
      include more surrounding lines to make the match unique. The count is data
      so the model knows how ambiguous it is.
---
that text appears {{count}} times in {{path}}, so I can't tell which one to change — include more of the surrounding lines so it matches only once
