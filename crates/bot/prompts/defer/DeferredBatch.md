---
id: DeferredBatch
type: prompt
annotations:
  sent_when: >-
    the user turn of a fired deferred-task batch — sent when a run_when wait
    resolves and its queued tasks run as one manager-tier Gary turn. The system
    prompt is the normal Discord manager-tier prompt; this supplies the situation
    and the asks.
  used_by:
    - file: defer/condition.rs
      function: compose_batch_prompt
  variables:
    server:
      source: the watched server's Kubernetes instance name
      contents: the server name, quoted in the opener
    trigger_note:
      source: >-
        one of the trigger-note prompts, selected by which condition fired and
        its outcome — StartupReady, StartupCrashed, StartupStopped,
        StartupTimedOut, StartupNotFound, StartupNotManaged, StartupUnchecked,
        OccupancyEmpty, OccupancyIdle, OccupancyNotFound, OccupancyNotManaged,
        or OccupancyTimedOut (rendered in defer/watcher.rs).
      contents: >-
        a predicate phrase that grammatically completes 'The server "<server>"
        …' — e.g. "is now empty — no players are connected".
    tasks:
      source: the drained DeferredTask list, numbered and joined in compose_batch_prompt
      contents: >-
        the queued tasks as "<n>. <task>" lines joined by newlines, in the order
        they were queued.
  reasoning:
    - >-
      Frames a fired batch as one user turn: names what woke it (trigger_note),
      lists the queued tasks, and tells Gary to run them only if they still make
      sense — and to investigate rather than blindly run if the server came up
      unhealthy. The "nobody is waiting on a live reply, post a summary" framing
      is load-bearing: the batch runs unattended, so Gary must report to the
      channel instead of assuming someone will read a live response.
    - >-
      The numbered task list is computed data and enters through {{tasks}}; only
      the opener and the closing instructions are prompt text.
---
[Deferred work] The server "{{server}}" {{trigger_note}}. A little while ago, these were queued to run once that happened:
{{tasks}}
Carry out the queued tasks now, if they still make sense for the situation. If the server failed to come up or is unhealthy, don't run them blindly — look into what went wrong (read the logs), fix it if you safely can, and say plainly what happened. Nobody is waiting on a live reply, so when you're done, post a short plain-language summary to the channel of what you did (and anything you couldn't). If a task no longer makes sense, say so instead of forcing it.
