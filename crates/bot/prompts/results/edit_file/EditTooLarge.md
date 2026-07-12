---
id: EditTooLarge
type: prompt
annotations:
  sent_when: >-
    tool result when the target file is too big for edit_file to change safely
    with an in-place text replacement
  used_by:
    - file: discord/gary/tools.rs
      function: format_edit
  variables:
    path:
      source: the file path the edit targeted
      contents: the file path
  reasoning:
    - >-
      Redirects the model to the right tool (write_file for a full rewrite) when
      an in-place edit isn't safe, so it changes approach rather than retrying an
      edit that will keep being refused.
---
{{path}} is too big to edit safely this way — rewrite the whole file with write_file instead
