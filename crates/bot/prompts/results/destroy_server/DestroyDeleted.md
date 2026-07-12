---
id: DestroyDeleted
type: prompt
annotations:
  sent_when: >-
    tool result when destroy_server, after the human confirmed, permanently
    deletes the server and its world
  used_by:
    - file: discord/gary/tools.rs
      function: format_destroy
  variables:
    server:
      source: the server that was deleted
      contents: the server name
  reasoning:
    - >-
      States the finality plainly (the world is gone too) so the model doesn't
      soften it or imply recoverability — this is the irreversible outcome.
---
deleted {{server}} and its world
