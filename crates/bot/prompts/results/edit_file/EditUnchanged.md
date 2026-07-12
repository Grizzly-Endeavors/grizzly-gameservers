---
id: EditUnchanged
type: prompt
annotations:
  sent_when: >-
    tool result when edit_file's old_text and new_text are identical, so there's
    nothing to change
  used_by:
    - file: discord/gary/tools.rs
      function: format_edit
  variables:
    path:
      source: the file path the edit targeted
      contents: the file path
  reasoning:
    - >-
      Flags a no-op edit so the model doesn't think it changed something it
      didn't, and can re-check its intended new_text.
---
the old and new text are identical, so there's nothing to change in {{path}}
