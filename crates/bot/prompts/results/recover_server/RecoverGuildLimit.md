---
id: RecoverGuildLimit
type: prompt
annotations:
  sent_when: >-
    tool result when recover_server is refused because recreating the archived
    server would put its owning community over one of its resource caps
  used_by:
    - file: discord/gary/tools.rs
      function: format_recover
  variables:
    detail:
      source: describe_limit_refusal over the carried LimitRefusal figures
      contents: one sentence naming the cap that would be broken and its figures
  reasoning:
    - >-
      The archive is untouched, and the wording says so — a friend must never
      read a cap refusal as their world being lost. Covers both the count and
      resource caps in one result because the recover remedies are the same:
      free room (destroy, archive, stop, or shrink something) and retry, or ask
      the operator for more.
---
the world is safe in its archive, but nothing was recovered — {{detail}}. free up room first (destroy or archive an unused server, or stop or shrink a running one), then try again — or suggest asking the operator to raise the limit
