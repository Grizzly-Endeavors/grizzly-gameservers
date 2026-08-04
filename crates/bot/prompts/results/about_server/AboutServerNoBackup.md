---
id: AboutServerNoBackup
type: prompt
annotations:
  sent_when: >-
    appended to an about_server result when no backup has ever been recorded for
    this server, or the event log is unavailable to say
  used_by:
    - file: discord/gary/tools.rs
      function: history_lines
  reasoning:
    - >-
      Deliberately does not claim the server is unprotected. The event log is
      optional and degrades to silence when the database is absent, so "no record"
      and "never backed up" are indistinguishable here — asserting the latter
      would talk someone into an unnecessary restore.
---
Last backup: no record of one. This may mean it has never been backed up, or only that the record is unavailable — don't assume the world is unprotected without checking.
