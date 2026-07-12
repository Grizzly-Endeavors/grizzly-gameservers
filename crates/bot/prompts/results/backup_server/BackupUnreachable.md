---
id: BackupUnreachable
type: prompt
annotations:
  sent_when: >-
    tool result when backup_server can't reach the server to snapshot it
  used_by:
    - file: discord/gary/tools.rs
      function: format_backup
  variables:
    server:
      source: the server that was targeted
      contents: the server name
  reasoning:
    - >-
      A transient reach failure (worth retrying), distinct from the not-running
      case, so the model retries rather than telling the user to start a server
      that may already be up.
---
I couldn't reach {{server}} to back it up — worth trying again in a moment
