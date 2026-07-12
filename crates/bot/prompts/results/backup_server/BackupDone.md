---
id: BackupDone
type: prompt
annotations:
  sent_when: >-
    tool result when backup_server successfully snapshots a running server
  used_by:
    - file: discord/gary/tools.rs
      function: format_backup
  variables:
    server:
      source: the server that was backed up
      contents: the server name
    size:
      source: the snapshot size, formatted by human_size
      contents: the human-readable backup size (e.g. "42 MB")
  reasoning:
    - >-
      Confirms the backup and names the tool that undoes to it (restore_server),
      so the model knows the recovery path. The size is data entering through the
      variable.
---
backed up {{server}} ({{size}}); restore_server can roll it back to this point
