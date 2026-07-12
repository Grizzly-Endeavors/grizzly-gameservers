---
id: LifecyclePaused
type: prompt
annotations:
  sent_when: >-
    tool result when a stop/pause lifecycle op succeeds — the supervisor
    confirmed the server is paused with its world saved
  used_by:
    - file: discord/gary/tools.rs
      function: format_supervisor
  variables:
    server:
      source: the target server's instance name
      contents: the server name
  reasoning:
    - >-
      Reassures via the result that pausing is non-destructive (world saved,
      kept warm), so Gary relays that a paused server can be resumed rather than
      implying it was torn down.
---
paused {{server}}; world saved and kept warm
