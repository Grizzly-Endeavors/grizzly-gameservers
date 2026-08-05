---
id: CreateGuildLimitResources
type: prompt
annotations:
  sent_when: >-
    tool result when create_server is refused because the new server's CPU or
    memory limits would push the community's live total over its allowance
  used_by:
    - file: discord/gary/tools.rs
      function: refusal_reply
  variables:
    detail:
      source: describe_limit_refusal over the carried LimitRefusal figures
      contents: >-
        one sentence naming what the new server needs, what the community's
        live servers already hold, and the allowance it would break
  reasoning:
    - >-
      Only live servers count against the CPU/memory allowance, so stopping or
      shrinking one genuinely frees room — different remedies from the
      server-count cap, which is why this is its own result. The escalation
      path stays: the operator can raise the allowance.
---
nothing was created — {{detail}}. stopping or shrinking another server frees room (only running servers count against the allowance), or suggest asking the operator for more
