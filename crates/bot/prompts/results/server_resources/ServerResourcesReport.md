---
id: ServerResourcesReport
type: prompt
annotations:
  sent_when: tool result when server_resources finds the instance
  used_by:
    - file: discord/gary/tools.rs
      function: format_resources
  variables:
    server:
      source: the instance name the tool was called with
      contents: the instance name
    cpu:
      source: >-
        the declared CPU request and limit off the live pod, plus actual usage
        from metrics-server, rendered by format_cpu_line
      contents: >-
        a one-line summary; unreadable figures render as "unknown", never as
        zero, since a metrics outage is not idleness
    memory:
      source: the same three memory figures, rendered by format_memory_line
      contents: as cpu
    disk:
      source: the PVC's provisioned capacity
      contents: a human size, or "unknown" when the claim hasn't reported one
    origin:
      source: >-
        the rendered ServerResourcesOverridden or ServerResourcesCatalog,
        chosen on whether the instance carries a resource override
      contents: >-
        which of the two chose these figures, and — when it was a resize — what
        this game's own sizing is; never blank, because "who chose these
        numbers" is the first thing asked after seeing them
  reasoning:
    - >-
      Reserved, maximum and actual on one line per resource, because the useful
      judgement is the relationship between them and separating them into three
      readings invites reporting only one.
    - >-
      Names whether the figures came from a previous resize or the game's
      defaults, which is what tells the model whether a change would be the
      first one or an adjustment.
    - >-
      A resized server's line also carries the game's own figures, so a stale
      override is distinguishable from a live one and going back to the defaults
      is a copy rather than a guess. See ServerResourcesOverridden.
---
{{server}} resources:
cpu: {{cpu}}
memory: {{memory}}
world storage: {{disk}}
these are {{origin}}
