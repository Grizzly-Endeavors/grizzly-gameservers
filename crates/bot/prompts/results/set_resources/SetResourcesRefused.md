---
id: SetResourcesRefused
type: prompt
annotations:
  sent_when: >-
    tool result when the node cannot satisfy the requested size — typically a
    memory limit below what the server is currently using
  used_by:
    - file: discord/gary/tools.rs
      function: format_resize
  variables:
    server:
      source: the instance name that was resized
      contents: the instance name
    detail:
      source: the kubelet's own message from the PodResizePending condition
      contents: >-
        the cluster's explanation, passed through verbatim — the model relays
        the gist rather than the wording
  reasoning:
    - >-
      The saved override still stands, so this ends up applied on the next
      restart. Saying so prevents the model reporting a clean failure when the
      change is actually pending a restart.
    - >-
      Names the usual cause, because the common case (shrinking memory below
      live usage) has an obvious next step the model can offer.
---
couldn't resize {{server}} live: {{detail}}. the setting is saved and will apply next time it starts. if you were lowering memory, the server is currently using more than the new limit — either leave it higher or restart it to apply.
