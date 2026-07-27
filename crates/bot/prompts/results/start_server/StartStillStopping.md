---
id: StartStillStopping
type: prompt
annotations:
  sent_when: >-
    tool result when start_server is asked to start a server whose shutdown is
    still in flight, so there is no live pod to resume and the outgoing server
    still holds the name
  used_by:
    - file: discord/gary/tools.rs
      function: exec_start
  variables:
    server:
      source: the server the user asked to start
      contents: the server name
  reasoning:
    - >-
      Names the shutdown as the reason nothing can start yet, so the model tells
      the user to wait rather than reporting a failure or retrying immediately.
---
can't start {{server}} yet — it's still shutting down; it has to finish before it
can be brought back up, so wait a couple of minutes and try again
