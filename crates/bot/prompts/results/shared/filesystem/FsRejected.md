---
id: FsRejected
type: prompt
annotations:
  sent_when: >-
    tool result when the server accepted the filesystem request but rejected it
    with a reason (e.g. a bad path)
  used_by:
    - file: discord/gary/tools.rs
      function: fs_result
  variables:
    message:
      source: the supervisor's rejection reason for the file op
      contents: the raw reason text the server returned, appended after the colon
  reasoning:
    - >-
      Passes the server's own rejection reason through as data so Gary relays the
      specific problem (which the model can often fix, like correcting a path)
      rather than a generic failure.
---
that didn't work: {{message}}
