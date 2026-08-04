---
id: WatchdogUnreachable
type: prompt
annotations:
  sent_when: >-
    fills the WatchdogIncident trigger-note slot when the supervisor's control
    API has been unreachable for consecutive sampler ticks while the pod stayed
    up and labelled running — a wedged supervisor
  used_by:
    - file: watchdog/respond.rs
      function: trigger_note
  variables:
    minutes:
      source: the unreachable streak's length in wall-clock minutes (streak ticks × sampler interval)
      contents: how long the control API has been unanswering, rendered as a bare integer
  reasoning:
    - >-
      Warns that the model's own tools go through the same unreachable API, so
      failed status/logs/restart calls are the symptom itself, not a reason to
      retry harder. That reframing steers the turn toward reporting and
      escalation rather than a loop of failing tool calls. Predicate clause
      completing 'The server "<name>" …'.
---
has stopped answering its control API for the last {{minutes}} minutes even though its pod is still up and claims a running process — your own tools (status, logs, file reads, restart) go through that same API, so if they fail against this server, that failure is the symptom, not a fluke worth retrying
