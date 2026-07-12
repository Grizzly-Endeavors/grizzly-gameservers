---
id: ConfirmNoChannel
type: prompt
annotations:
  sent_when: >-
    tool result when a confirmation-gated backup action (archive or restore)
    can't post its confirmation prompt in the channel, so the action never runs
  used_by:
    - file: discord/gary/tools.rs
      function: confirm_destructive
  reasoning:
    - >-
      The shared no-channel fallback for the archive/restore confirm flow — worded
      "I didn't do anything" because the action is unknown at this layer (distinct
      from destroy's "I didn't delete anything"). Makes clear the gate never ran,
      so nothing was changed.
---
I couldn't post a confirmation prompt in this channel, so I didn't do anything.
