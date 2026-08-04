---
id: AboutServerNoAction
type: prompt
annotations:
  sent_when: >-
    appended to an about_server result when the event log holds no lifecycle
    action for this server, or is unavailable
  used_by:
    - file: discord/gary/tools.rs
      function: history_lines
  reasoning:
    - >-
      Same reasoning as AboutServerNoBackup: an absent record is not evidence that
      nothing happened, and the wording must not let the model treat it as such.
---
Last change: no record of one. The history may simply be unavailable, so this doesn't mean nothing has happened to the server.
