---
id: UnknownTool
type: prompt
annotations:
  sent_when: >-
    tool result when the model calls a tool name that doesn't exist (not a
    tier-gated one, just unknown), or a real tool that a fixed-directory run
    (gary-local) never offers because it needs the cluster — returned from the
    tier gate, from dispatch's fixed-directory gate, and from either
    dispatcher's fallthrough
  used_by:
    - file: discord/gary/tools.rs
      function: dispatch
    - file: discord/gary/tools.rs
      function: dispatch_memory
    - file: discord/gary/tools.rs
      function: dispatch_mutating
    - file: discord/gary/tools.rs
      function: tier_refusal
  variables:
    name:
      source: the tool name the model called
      contents: the unrecognized tool name, quoted inline
  reasoning:
    - >-
      Names the bad tool back to the model so it corrects to a real one instead
      of retrying the same hallucinated name. Distinct from the tier refusals,
      which name a real-but-out-of-tier tool.
---
'{{name}}' isn't a tool I have.
