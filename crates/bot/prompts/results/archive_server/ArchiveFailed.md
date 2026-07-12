---
id: ArchiveFailed
type: prompt
annotations:
  sent_when: >-
    tool result when archive_server couldn't complete the archive cleanly, so no
    storage was released
  used_by:
    - file: discord/gary/tools.rs
      function: format_archive
  variables:
    server:
      source: the server that was targeted
      contents: the server name
  reasoning:
    - >-
      Reassures that a failed archive left the server intact (nothing released),
      and marks it retryable, so the model can try again without fearing partial
      damage.
---
I couldn't archive {{server}} cleanly, so nothing was released — worth trying again shortly
