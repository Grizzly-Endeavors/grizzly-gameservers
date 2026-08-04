---
id: WatchdogIncident
type: prompt
annotations:
  sent_when: >-
    the user turn of a proactive watchdog incident — sent when the detector
    fires an incident and the responder runs a manager-tier Gary turn (watchdog
    mode "on"). The system prompt is the normal Discord manager-tier prompt;
    this supplies what the monitoring saw and what to do about it.
  used_by:
    - file: watchdog/respond.rs
      function: compose_incident_prompt
  variables:
    server:
      source: the incident's Kubernetes instance name
      contents: the server name, quoted in the opener
    trigger_note:
      source: >-
        one of the watchdog trigger-note prompts — WatchdogCrashLoop,
        WatchdogCrashed, WatchdogUnreachable, or WatchdogMemoryPressure —
        selected by the incident's facts and rendered in watchdog/respond.rs.
      contents: >-
        a predicate phrase that grammatically completes 'The server "<server>"
        …', carrying the fired signal's numbers — e.g. "keeps crashing — 2
        unexpected exits in the last 15 minutes".
  reasoning:
    - >-
      Opens by saying nobody asked: the model must understand this is its own
      monitoring speaking, so there is no requester to clarify intent with and
      no live reader mid-turn. The "post a short plain-language summary" close
      is the same load-bearing framing DeferredBatch carries — an unattended
      turn must report to the channel, not assume someone saw a live reply.
    - >-
      Investigate-before-touching is spelled out (status and logs first)
      because the incident facts alone name a symptom, not a cause. The "don't
      experiment" escape hatch bounds an unattended manager-tier turn: a wrong
      guess applied to a live server is worse than a plain report, and saying
      so out loud is what licenses the model to stop.
---
[Proactive check] Nobody asked about this — your own monitoring flagged it. The server "{{server}}" {{trigger_note}}.
Look into it now. Start by checking the server's status and reading its logs to find out what is actually wrong. If the cause is clear and the fix is safe — restarting the server, or reverting a recent config change — fix it and verify it worked. If you are not confident about the cause or the fix, don't experiment on a live server: say plainly what you found and leave it for a human.
Nobody is waiting on a live reply, so when you're done, post a short plain-language summary to the channel: what was wrong, what you did, and anything that still needs attention.
