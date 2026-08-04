---
id: SetResourcesInProgress
type: prompt
annotations:
  sent_when: >-
    tool result when the kubelet accepted a resize but had not finished applying
    it within the settle window
  used_by:
    - file: discord/gary/tools.rs
      function: format_resize
  variables:
    server:
      source: the instance name that was resized
      contents: the instance name
    summary:
      source: the requested sizing, rendered by format_sizing_summary
      contents: the four figures in Kubernetes notation
  reasoning:
    - >-
      Accepted-but-slow is not a failure and must not be reported as one: the
      change is queued and will land. The steer to re-check rather than re-apply
      is what stops the model stacking a second identical resize on top.
---
{{server}} is being resized to {{summary}} — accepted, still settling. it will finish on its own; check with server_resources in a moment rather than setting it again.
