---
id: WatchdogCrashLoop
type: prompt
annotations:
  sent_when: >-
    fills the WatchdogIncident trigger-note slot when the restart counter
    climbed past the crash-loop threshold — the game process keeps dying and
    the supervisor keeps relaunching it
  used_by:
    - file: watchdog/respond.rs
      function: trigger_note
  variables:
    restarts:
      source: the summed positive restart-counter deltas across the detection window
      contents: the count of unexpected exits, rendered as a bare integer
    minutes:
      source: the detection window's width in wall-clock minutes (window ticks × sampler interval)
      contents: the window length, rendered as a bare integer
  reasoning:
    - >-
      Says the supervisor keeps relaunching it so the model knows the server
      may look "up" right now — the problem is the pattern, not the current
      moment. Names the usual causes (a bad config change, a corrupted world)
      to steer the first diagnosis the same way StartupCrashed does. Predicate
      clause completing 'The server "<name>" …'.
---
keeps crashing — {{restarts}} unexpected exits in the last {{minutes}} minutes, each relaunched by the supervisor, so it may look up right now (a bad config change or a corrupted world are the usual causes)
