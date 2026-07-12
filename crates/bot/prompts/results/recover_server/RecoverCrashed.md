---
id: RecoverCrashed
type: prompt
annotations:
  sent_when: >-
    tool result when recover_server recreates an archived server but it crashes
    coming back up
  used_by:
    - file: discord/gary/tools.rs
      function: format_recover
  variables:
    name:
      source: the recovered server's name
      contents: the server name
    address:
      source: the recovered server's advertised address
      contents: the host:port players would connect to
  reasoning:
    - >-
      Names the likely cause (the archived data) and next steps (read logs, or
      escalate), mirroring the restore crash result — the archived world itself
      may be at fault.
---
recovered {{name}} at {{address}}, but it crashed coming back up — read its logs (the archived data may be the cause), or ping an operator
