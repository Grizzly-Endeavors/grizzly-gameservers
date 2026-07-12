---
id: RecoverNoArchiveHere
type: prompt
annotations:
  sent_when: >-
    tool result when recover_server is asked for an archive name that isn't in
    the caller's scope, resolved during the pre-recovery archive lookup
  used_by:
    - file: discord/gary/tools.rs
      function: exec_recover
  variables:
    name:
      source: the archive name the model asked to recover
      contents: the unresolved archive name
  reasoning:
    - >-
      The lookup-time not-found (distinct wording from the outcome-time
      RecoverNoSuchArchive) — steers the model to re-list archives rather than
      retrying a name that isn't visible in this scope.
---
there's no archived server named {{name}} here — check list_archives
