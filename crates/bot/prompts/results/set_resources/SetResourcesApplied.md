---
id: SetResourcesApplied
type: prompt
annotations:
  sent_when: >-
    tool result when set_resources changed a running server in place and the
    kubelet confirmed the container took the new figures
  used_by:
    - file: discord/gary/tools.rs
      function: format_resize
  variables:
    server:
      source: the instance name that was resized
      contents: the instance name
    summary:
      source: the applied sizing, rendered by format_sizing_summary
      contents: the four figures in Kubernetes notation
  reasoning:
    - >-
      States that nobody was disconnected. That is the feature, and without it
      the model hedges to the friend about whether the server bounced.
    - >-
      Says the change persists across restarts, so the model doesn't warn about
      it reverting — the override is recorded before the patch is issued.
---
{{server}} is now running on {{summary}}, applied live — the server did not restart and nobody was disconnected. this sticks across future restarts.
