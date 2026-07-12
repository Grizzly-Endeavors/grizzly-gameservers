---
id: ChangeRestartedUnverified
type: prompt
annotations:
  sent_when: >-
    tool result when a config-change restart's readiness check itself errored, so
    the guardrail can't tell whether the server came back — the change is left in
    place rather than rolled back on a maybe-fine server
  used_by:
    - file: discord/gary/tools.rs
      function: verify_change
  variables:
    server:
      source: the server whose config change was restarted
      contents: the server name
  reasoning:
    - >-
      Honest uncertainty: the auto-verify couldn't determine health, so it says
      so and points to the logs rather than claiming success or destroying a
      possibly-healthy server with a needless rollback.
---
restarted {{server}}, but I couldn't tell whether it came back up — check its logs
