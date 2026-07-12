---
id: EditNoMatch
type: prompt
annotations:
  sent_when: >-
    tool result when edit_file's old_text isn't found verbatim in the target
    file, so no change was made
  used_by:
    - file: discord/gary/tools.rs
      function: format_edit
  variables:
    path:
      source: the file path the edit targeted
      contents: the file path
    server:
      source: the server the file lives on
      contents: the server name
  reasoning:
    - >-
      Tells the model exactly how to recover — re-read the file and copy the
      current text verbatim, whitespace and all — because a near-miss on
      whitespace is the usual cause. A soft failure that steers to a fix, not a
      dead end.
---
I couldn't find that exact text in {{path}} on {{server}} — read the file again and copy the current text verbatim, whitespace and all
