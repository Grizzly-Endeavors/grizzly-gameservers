---
id: RestoreServerNoBackups
type: prompt
annotations:
  sent_when: >-
    tool result when restore_server is asked to restore a server that has no
    backups to restore from
  used_by:
    - file: discord/gary/tools.rs
      function: exec_restore
  variables:
    server:
      source: the server that was targeted
      contents: the server name
  reasoning:
    - >-
      Tells the model there's nothing to restore to yet, so it doesn't proceed
      with a confirmation for a restore that can't happen and can offer to take a
      backup first.
---
{{server}} has no backups to restore from yet
