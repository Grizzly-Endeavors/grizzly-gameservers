---
id: Escalate
type: tool
tool_schema:
  message:
    type: string
    description: >-
      What to say to the person who asked, in your own voice — what you found,
      what you tried, and that a human is picking it up. This is sent to them
      as your reply, so write it for them, not for an operator.
  reason:
    type: string
    description: >-
      What the operator needs to know: what you believe is wrong, what you
      already ruled out, and what you need from a human that you can't do
      yourself. Written for someone technical who wasn't watching.
  server:
    type: string
    description: >-
      The server this is about, if it is about one. Omit when the problem isn't
      tied to a particular server.
    optional: true
annotations:
  sent_when: >-
    offered on the Discord surface at every access tier, including read-only
    callers and DMs.
  used_by:
    - file: discord/gary/tools.rs
      function: tool_table
    - file: discord/gary/tools.rs
      function: dispatch
    - file: discord/gary/tools.rs
      function: exec_escalate
  reasoning:
    - >-
      The design's escalation exit (docs/design/00-overview.md, guardrail 3),
      made agent-invocable. Before this the only way out was exhausting the
      round budget, so a model that had run out of ideas early either thrashed
      or answered with a shrug. The body's job is to make stopping feel like a
      correct outcome rather than a failure, since a model that reads escalation
      as giving up will avoid it and thrash instead.
    - >-
      The body states that the turn ends on this call. Without that the model
      treats it as a notification and keeps planning work it will never get to
      run, which produces a reply that contradicts the operator notice.
    - >-
      message and reason are separate because they have different readers — a
      non-technical friend and an operator. Collapsing them would force one
      register on both, and the repo's error-message rule already separates
      these audiences everywhere else.
    - >-
      server is optional and the tool is deliberately outside the
      targets_existing_server scope gate: an escalation about a server the
      caller can't see must still reach an operator rather than being refused
      as "no such server". It is the only locator a Discord handoff has — the
      surface context carries none — so it lands on both the operator notice
      and the event row's instance column.
---
Hand the problem to a human and stop working on it. Use this when you have run out of things to try, when the fix needs access or a judgement call you don't have, or when acting on your best guess could make things worse. Stopping here is the right answer in those cases — it is better than guessing on a live server, and much better than repeating things that have already failed. Prefer it early: if two attempts at the same thing have not worked, a third usually won't either. Your turn ends the moment you call this, so say everything you want to say in `message` — you will not get another chance to reply, and any other tool you were planning to call will not run. Don't promise to follow up or check back; you can't, and the person will be told a human is looking.
