---
id: BackupNotRunning
type: prompt
annotations:
  sent_when: >-
    tool result when backup_server is asked to back up a server that isn't
    running, so there's no live world to snapshot
  used_by:
    - file: discord/gary/tools.rs
      function: format_backup
  variables:
    server:
      source: the server that was targeted
      contents: the server name
  reasoning:
    - >-
      Explains why the backup can't run (nothing live) and the fix (start it
      first), so the model offers to start rather than reporting an opaque
      failure.
---
{{server}} isn't running, so there's nothing live to back up — start it first
