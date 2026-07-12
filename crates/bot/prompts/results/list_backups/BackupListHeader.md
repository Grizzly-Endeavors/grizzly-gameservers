---
id: BackupListHeader
type: prompt
annotations:
  sent_when: >-
    tool result heading a list_backups listing when a server has one or more
    backups
  used_by:
    - file: discord/gary/tools.rs
      function: format_backup_list
  variables:
    server:
      source: the server whose backups were listed
      contents: the server name
    lines:
      source: the backup entries, each formatted and joined by newlines in format_backup_list
      contents: >-
        one line per backup — "<created_at> (<size>)" — joined by newlines; the
        per-entry formatting is data and stays in code
  reasoning:
    - >-
      The header (and its newest-first ordering note) is prompt text; the backup
      entries are data joined in code and enter through the variable.
---
backups of {{server}} (newest first):
{{lines}}
