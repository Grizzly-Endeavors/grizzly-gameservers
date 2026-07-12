---
id: BackupsNotConfigured
type: prompt
annotations:
  sent_when: >-
    tool result when any backup/archive/restore/recover tool runs but backups
    aren't configured on this bot deployment — there's no storage to act against
  used_by:
    - file: discord/gary/tools.rs
      function: backups_not_configured
  reasoning:
    - >-
      Tells the model the whole save/restore capability is absent on this
      deployment, so Gary explains it plainly rather than pretending a backup
      happened or retrying against storage that isn't there.
---
backups aren't set up on this bot, so there's nothing to save or restore
