---
id: DeleteArchiveDone
type: prompt
annotations:
  sent_when: returned from delete_archive once the archives are gone
  used_by:
    - file: discord/gary/tools.rs
      function: format_delete_archive
  variables:
    name:
      source: the archive name the caller asked to delete
      contents: the server name the archives were stored under
    generations:
      source: how many archive rows were deleted
      contents: the number of saved copies removed, as a number
    size:
      source: the summed size of the deleted archives, formatted
      contents: how much storage was freed, like "1.2 GiB"
  reasoning:
    - >-
      Reports the count because it is usually more than the one copy the listing
      showed — list_archives collapses to the newest per name, so "delete the smp
      archive" removes generations the caller never saw. Saying so after the fact
      keeps the confirmation honest.
---
Deleted {{generations}} saved copy(ies) of {{name}} from cold storage, freeing {{size}}. That world can no longer be recovered.
