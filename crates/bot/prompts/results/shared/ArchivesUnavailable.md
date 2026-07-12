---
id: ArchivesUnavailable
type: prompt
annotations:
  sent_when: >-
    tool result when archive records can't be reached (the archive index is
    offline) while backups and restore still work — returned from the archive
    listing/create/recover paths
  used_by:
    - file: discord/gary/tools.rs
      function: archives_unavailable_text
  reasoning:
    - >-
      Scopes the outage precisely for the model: archives specifically are
      offline, but backups and restore still work — so Gary steers to those
      rather than declaring the whole feature dead. "try again later" marks it
      transient.
---
I can't track archives right now — my archive records are offline. Backups and restore still work; try archiving again later
