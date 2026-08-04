---
id: ServerResources
type: tool
params_from: NameParams
annotations:
  sent_when: >-
    offered on the Discord surface at every access tier, including read-only
    callers
  used_by:
    - file: discord/gary/tools.rs
      function: tool_table
    - file: discord/gary/tools.rs
      function: dispatch
    - file: discord/gary/tools.rs
      function: exec_server_resources
  reasoning:
    - >-
      The read half of resource management, and the thing to call before
      set_resources: without it the model guesses at current values and proposes
      a change relative to nothing.
    - >-
      Read-only tier because "is my server short on memory?" is a reasonable
      question for anyone playing on it, and the answer reveals nothing another
      guild could use. Changing the figures is the admin-gated half.
    - >-
      The body names both halves — what's reserved and what's actually being
      used — because the useful judgement is the gap between them, and a model
      shown only one of the two reliably reports the wrong one.
---
Show what one server is allowed to use and what it is actually using: its reserved and maximum CPU and memory, current usage against those, and how much disk its world storage was given. Call this before changing a server's resources, and when someone asks whether a server is short on memory or being throttled — the useful answer is the gap between what's reserved and what's being used, not either number alone.
