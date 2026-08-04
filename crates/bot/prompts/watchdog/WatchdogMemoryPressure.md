---
id: WatchdogMemoryPressure
type: prompt
annotations:
  sent_when: >-
    fills the WatchdogIncident trigger-note slot when whole-pod memory has been
    pressing against the declared limit for consecutive sampler ticks — a
    sustained pattern, not a spike
  used_by:
    - file: watchdog/respond.rs
      function: trigger_note
  variables:
    used_mib:
      source: actual whole-pod memory from metrics-server, converted to MiB
      contents: current usage, rendered as a bare integer in MiB
    limit_mib:
      source: the declared pod memory limit, converted to MiB
      contents: the limit, rendered as a bare integer in MiB
    percent:
      source: usage as a percentage of the limit, computed at detection time
      contents: the percentage, rendered as a bare integer without a % sign
  reasoning:
    - >-
      Names the consequence (an OOM kill ejects everyone mid-play) so the model
      weighs urgency correctly, and immediately bounds the safe playbook: the
      in-game footprint (view distance, loaded chunks) is fixable from inside;
      the pod's memory limit is not — that needs a human, so say so instead of
      fighting it. Predicate clause completing 'The server "<name>" …'.
---
has been running close to its memory limit for a sustained period — {{used_mib}} MiB of {{limit_mib}} MiB ({{percent}}%); if it keeps climbing the process will be OOM-killed, ejecting everyone mid-play. You can reduce the game's own footprint from inside (view distance, loaded chunks, pruning heavy mods) if the logs point somewhere clear, but the pod's memory limit itself is not yours to change — if the game genuinely needs more headroom, say so plainly and leave the resizing to a human
