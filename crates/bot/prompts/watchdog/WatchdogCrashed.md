---
id: WatchdogCrashed
type: prompt
annotations:
  sent_when: >-
    fills the WatchdogIncident trigger-note slot when the supervisor crossed
    its own crash threshold and stopped relaunching — the process is down and
    stays down
  used_by:
    - file: watchdog/respond.rs
      function: trigger_note
  variables:
    restarts:
      source: the supervisor's restart counter at detection (unexpected exits since the pod started)
      contents: the count of unexpected exits, rendered as a bare integer
  reasoning:
    - >-
      Distinct from WatchdogCrashLoop because the situation is worse and the
      playbook differs: nothing is coming back on its own, so the model must
      find the cause and then explicitly restart (or roll a config back) —
      "wait and see" is not an option. The likely-cause hint mirrors
      StartupCrashed's. Predicate clause completing 'The server "<name>" …'.
---
has crashed repeatedly and the supervisor has given up relaunching it — the process is down and will stay down until the cause is fixed and it is started again ({{restarts}} unexpected exits since its pod came up; a bad config change is the usual cause)
