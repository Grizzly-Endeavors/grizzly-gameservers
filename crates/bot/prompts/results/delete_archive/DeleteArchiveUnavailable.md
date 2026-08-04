---
id: DeleteArchiveUnavailable
type: prompt
annotations:
  sent_when: returned from delete_archive when the archive catalog isn't configured
  used_by:
    - file: discord/gary/tools.rs
      function: format_delete_archive
  reasoning:
    - >-
      Mirrors how the other archive tools report a missing catalog, so a database
      outage reads the same way whichever archive tool the model reached for.
---
Archives aren't available right now — the archive catalog isn't reachable, so nothing can be listed or deleted. Nothing was changed.
