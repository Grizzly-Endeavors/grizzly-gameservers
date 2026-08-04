---
id: SetResourcesRefused
type: prompt
annotations:
  sent_when: >-
    tool result when the change can't be made on the running container — the API
    refusing it outright (lowering a memory limit) or the kubelet reporting it
    infeasible (no room on the node)
  used_by:
    - file: discord/gary/tools.rs
      function: format_resize
  variables:
    server:
      source: the instance name that was resized
      contents: the instance name
    detail:
      source: >-
        the cluster's own message — the API's 422 body, or the kubelet's
        PodResizePending condition
      contents: >-
        the cluster's explanation, passed through verbatim — the model relays
        the gist rather than the wording
  reasoning:
    - >-
      The saved override still stands, so this ends up applied on the next
      restart. Saying so prevents the model reporting a clean failure when the
      change is actually pending a restart.
    - >-
      Names the rule rather than guessing at a cause. Lowering a memory limit is
      forbidden in place by Kubernetes outright — not conditional on current
      usage — so a message implying "use less and retry" would send the model
      chasing something that can't work. The next step is a restart, and it says
      so.
---
couldn't change {{server}} while it's running: {{detail}}. the setting is saved and takes effect the next time it starts, so nothing is lost. lowering a memory limit is the usual reason — that one can never be done live, by design, since applying it would mean restarting the server. if it needs to take effect now, restart it; otherwise leave it and it lands on the next start.
