---
id: OccupancyUnknown
type: prompt
annotations:
  sent_when: >-
    appended to a server_status result when the live player count couldn't be
    read — the reason clause explains why
  used_by:
    - file: discord/gary/tools.rs
      function: occupancy_line
  variables:
    reason:
      source: >-
        one of the OccupancyReason* prompts (NoCount / NotUp / NoConsole /
        Unread), selected by why the count couldn't be read, rendered and spliced
        into the parenthetical
      contents: a short phrase naming why occupancy is unknown
  reasoning:
    - >-
      Distinguishes "can't tell" from "zero players" so the model doesn't report
      an empty server when it simply couldn't read the count. The specific reason
      composes in through the variable. The leading newline separator is owned by
      code.
---
Players online: unknown ({{reason}})
