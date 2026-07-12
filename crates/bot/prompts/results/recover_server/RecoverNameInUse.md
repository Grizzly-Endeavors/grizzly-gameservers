---
id: RecoverNameInUse
type: prompt
annotations:
  sent_when: >-
    tool result when recover_server can't recreate an archive because a live
    server already has that name
  used_by:
    - file: discord/gary/tools.rs
      function: format_recover
  variables:
    name:
      source: the name that's already in use
      contents: the conflicting server name
  reasoning:
    - >-
      Redirects the model to start_server (the running server already exists), so
      it acts on the live one instead of trying to recover over a name that's
      taken.
---
a server named {{name}} is already running — use start_server instead
