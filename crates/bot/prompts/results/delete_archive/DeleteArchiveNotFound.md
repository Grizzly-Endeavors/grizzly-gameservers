---
id: DeleteArchiveNotFound
type: prompt
annotations:
  sent_when: returned from delete_archive when no archive of that name exists in the caller's scope
  used_by:
    - file: discord/gary/tools.rs
      function: format_delete_archive
  variables:
    name:
      source: the archive name the caller asked to delete
      contents: the name that matched nothing
  reasoning:
    - >-
      Points at list_archives rather than the server roster: an archived world has
      no server, so the listing the model would otherwise reach for can neither
      confirm nor deny the name.
---
There's no archive named {{name}} here. Use list_archives to see what's actually in cold storage — nothing was deleted.
