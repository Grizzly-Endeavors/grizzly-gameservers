---
id: LifecycleControlUnreachable
type: prompt
annotations:
  sent_when: >-
    tool result when a lifecycle op can't reach the server's supervisor controls
    at all
  used_by:
    - file: discord/gary/tools.rs
      function: format_supervisor
  variables:
    server:
      source: the target server's instance name
      contents: the server name
  reasoning:
    - >-
      Distinguishes an unreachable control channel from a not-yet-ready one;
      both are transient, but this one is a reach failure, worded "try again in a
      moment" so Gary retries rather than escalating.
---
I couldn't reach {{server}}'s controls right now — worth trying again in a moment
