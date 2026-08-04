---
id: SetResourcesRecorded
type: prompt
annotations:
  sent_when: >-
    tool result when set_resources changed a server that has no running pod, so
    there was nothing to resize in place
  used_by:
    - file: discord/gary/tools.rs
      function: format_resize
  variables:
    server:
      source: the instance name that was resized
      contents: the instance name
    summary:
      source: the recorded sizing, rendered by format_sizing_summary
      contents: the four figures in Kubernetes notation
  reasoning:
    - >-
      A shut-down server has no container to patch, but the override is saved
      and applied at render time — so this is a success, not a partial failure,
      and the wording has to say when it takes effect rather than implying it
      already did.
---
{{server}} is set to {{summary}}. it isn't running, so this takes effect the next time it starts.
