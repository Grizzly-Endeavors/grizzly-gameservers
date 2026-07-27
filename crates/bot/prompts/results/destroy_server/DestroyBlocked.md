---
id: DestroyBlocked
type: prompt
annotations:
  sent_when: >-
    tool result when destroy_server refused to continue because the server would
    not confirm it had stopped, so its world was deliberately left in place
  used_by:
    - file: discord/gary/tools.rs
      function: format_destroy
  variables:
    server:
      source: the server that was not deleted
      contents: the server name
  reasoning:
    - >-
      Makes clear that nothing was deleted, so the model never implies the
      destroy went through — a friend who believes a world is gone won't ask for
      it back.
    - >-
      Frames it as a deliberate refusal rather than a failure, because deleting
      storage out from under a server that may still be writing to it is the one
      outcome worth blocking.
---
did not delete {{server}} — it wouldn't confirm it had stopped, and deleting its
world while it may still be running risks losing it; nothing was deleted and the
destroy can be retried once the server is reachable again
