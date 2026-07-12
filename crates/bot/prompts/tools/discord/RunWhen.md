---
id: RunWhen
type: tool
tool_schema:
  name:
    type: string
    description: Exact server name to act on, as shown by `list_servers`.
  condition:
    type: enum
    description: >-
      When to run the task: `startup` (watch a (re)start finish — whether it
      comes up healthy or fails/stalls), `empty` (the moment no players are
      connected — for urgent changes as people log off), or `idle` (after it's
      been empty a few minutes — for no-rush tweaks).
    values:
      - startup
      - empty
      - idle
  task:
    type: string
    description: >-
      What to do once the condition is met, phrased the way you'd note it for
      yourself — e.g. "set difficulty to hard and restart", or "let them know
      it's up and healthy".
annotations:
  sent_when: offered on the Discord surface to managers and admins.
  used_by:
    - file: discord/gary/tools.rs
      function: manager_tools
    - file: discord/gary/tools.rs
      function: dispatch_mutating
  reasoning:
    - >-
      The deferred-task scheduler (ADR-007). The body is long on purpose: it
      teaches the model that scheduling returns immediately, that it must carry
      the task out itself when the condition fires (there is no notification
      system, so it must not promise to ping), and how to choose between the
      three conditions.
    - >-
      condition is a closed enum so the model can't invent a fourth trigger; the
      generated RunWhenCondition is narrowed to the domain Condition at the
      dispatch boundary, which keeps the Redis key layout the sole owner of the
      wire strings.
---
Schedule a task to run later, once a server reaches a condition, instead of blocking the conversation now or making the user wait around. Good for slow jobs (e.g. right after start_server or restart_server) and for changes that need a restart while people are still playing. The `condition` is one of: "startup" — watch a (re)starting server settle so you can confirm it came up healthy, or catch a boot that crashes, loops, or stalls; "empty" — fire the moment no players are connected, for a change wanted ASAP as people log off; "idle" — fire after the server has been empty for a few minutes, for a no-rush tweak. Pick empty when it's urgent, idle when it can wait; ask the user if it's unclear. Returns immediately — you then carry the task out yourself when the condition is met and report back in the channel. There is no notification system and you can't ping anyone, so don't promise to; you do the work and come back with the result. Only works on games that report a live player count for the empty/idle conditions.
