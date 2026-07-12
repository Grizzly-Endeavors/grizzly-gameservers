---
id: BackupListEmpty
type: prompt
annotations:
  sent_when: >-
    tool result when list_backups finds a server has no backups yet
  used_by:
    - file: discord/gary/tools.rs
      function: format_backup_list
  variables:
    server:
      source: the server whose backups were listed
      contents: the server name
  reasoning:
    - >-
      States there's nothing to list yet so the model reports it plainly and can
      offer to take a first backup, rather than implying an error.
---
{{server}} has no backups yet
