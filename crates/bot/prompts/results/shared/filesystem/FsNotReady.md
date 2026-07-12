---
id: FsNotReady
type: prompt
annotations:
  sent_when: >-
    tool result when a filesystem op (browse/read/write/edit/restore) reaches a
    pod that isn't ready to serve file operations yet
  used_by:
    - file: discord/gary/tools.rs
      function: fs_result
  variables:
    server:
      source: the target server's instance name
      contents: the server name
  reasoning:
    - >-
      Transient not-ready state for the file path (parallel to the lifecycle
      not-ready result but for file ops), worded "try again shortly" so Gary
      waits rather than treating it as a hard failure.
---
{{server}} isn't ready to work with yet — try again shortly
