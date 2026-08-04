---
id: AboutServerLastBackup
type: prompt
annotations:
  sent_when: appended to an about_server result when the server has a recorded backup
  used_by:
    - file: discord/gary/tools.rs
      function: history_lines
  variables:
    when:
      source: the created_at of the newest backup_taken event for this server
      contents: the timestamp of the most recent successful backup
  reasoning:
    - >-
      The question this answers is "is it safe to change something", which is
      exactly when Gary is about to edit a config or restart. Having the answer in
      hand means it can say "last backup was an hour ago" instead of offering to
      take one every time.
---
Last backup: {{when}}.
