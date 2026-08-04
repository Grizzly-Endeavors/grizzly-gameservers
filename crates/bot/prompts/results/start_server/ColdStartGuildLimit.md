---
id: ColdStartGuildLimit
type: prompt
annotations:
  sent_when: >-
    tool result when a cold start is refused because bringing the stopped
    server's pod back would push the community's live CPU/memory total over its
    allowance
  used_by:
    - file: discord/gary/tools.rs
      function: exec_cold_start
  variables:
    server:
      source: the instance name the user asked to start
      contents: a server name
    detail:
      source: describe_limit_refusal over the carried LimitRefusal figures
      contents: >-
        one sentence naming what starting the server needs, what the
        community's live servers already hold, and the allowance it would break
  reasoning:
    - >-
      Only pod-recreating starts are gated (a warm resume never left the
      aggregate), and the world is untouched — the wording must not read as the
      server being gone. The way out mirrors the resize refusal: free room by
      stopping or shrinking a live server, or escalate to the operator.
---
{{server}} wasn't started (its world is untouched) — {{detail}}. stopping or shrinking one of the community's running servers frees room, or suggest asking the operator for a bigger allowance
