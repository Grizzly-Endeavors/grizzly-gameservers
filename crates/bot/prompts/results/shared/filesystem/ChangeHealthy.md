---
id: ChangeHealthy
type: prompt
annotations:
  sent_when: >-
    tool result when a config change was applied by a restart and the server came
    back up healthy on the first try (no rollback needed)
  used_by:
    - file: discord/gary/tools.rs
      function: verify_change
  variables:
    server:
      source: the server that was restarted
      contents: the server name
  reasoning:
    - >-
      Confirms the change is verified-good (not just applied), so Gary can tell
      the user the edit stuck and the server is healthy — the happy path of the
      snapshot→apply→verify guardrail.
---
restarted {{server}} and it came back up healthy — the change is good
