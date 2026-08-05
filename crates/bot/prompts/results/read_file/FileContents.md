---
id: FileContents
type: prompt
annotations:
  sent_when: >-
    tool result carrying a file's contents back to the model after read_file
  used_by:
    - file: discord/gary/tools.rs
      function: format_file
  variables:
    path:
      source: the file path that was read
      contents: the file path, in the header
    note:
      source: >-
        the FileTruncatedNote prompt when the file was truncated, else empty —
        assembled in code with a code-owned leading space separator
      contents: >-
        empty when the whole file was returned, otherwise a leading-space-
        prefixed parenthetical (rendered from FileTruncatedNote) marking that the
        content was cut short
    content:
      source: the file's text as returned by the supervisor
      contents: the raw file contents, verbatim
  reasoning:
    - >-
      Labels the payload as file contents and flags truncation inline so the
      model doesn't treat a cut-off file as complete. The content and the
      optional note are data entering through variables; only the "contents of"
      framing is prompt text.
    - >-
      Fenced because a file on a game's PVC is not platform-authored text: the
      game writes it, and much of what a game writes is whatever its players
      typed — names, signs, chat history, mod output. The markers are matched by
      crate::untrusted, which removes any line of the file that tries to write
      one.
---
contents of {{path}}{{note}}:
-----BEGIN UNTRUSTED FILE-----
{{content}}
-----END UNTRUSTED FILE-----
