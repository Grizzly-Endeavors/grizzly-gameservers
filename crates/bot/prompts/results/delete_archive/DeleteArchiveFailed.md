---
id: DeleteArchiveFailed
type: prompt
annotations:
  sent_when: returned from delete_archive when an archive object couldn't be removed from storage
  used_by:
    - file: discord/gary/tools.rs
      function: format_delete_archive
  variables:
    name:
      source: the archive name the caller asked to delete
      contents: the archive that was being deleted
  reasoning:
    - >-
      States that the archive is still whole. The delete stops at the first object
      it can't remove and leaves the index untouched, so a retry is safe and
      recovery still works — without saying so the model would report a
      half-deleted archive and talk the caller out of the retry that fixes it.
---
Couldn't delete the {{name}} archive — storage didn't accept the deletion, so it's been left alone rather than half-removed. It can still be recovered, and it's safe to try deleting again.
