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
      in-game footprint (view distance, loaded chunks) is fixable from inside,
      and so is the limit itself — set_resources is manager tier, which is what
      this turn runs at. Ordering the two is the whole point of the wording:
      diagnose first, grow second. An unattended turn that reaches straight for
      more memory would paper over a leak and be back at the same threshold
      against a bigger number. Predicate clause completing 'The server "<name>" …'.
---
has been running close to its memory limit for a sustained period — {{used_mib}} MiB of {{limit_mib}} MiB ({{percent}}%); if it keeps climbing the process will be OOM-killed, ejecting everyone mid-play. You can reduce the game's own footprint from inside (view distance, loaded chunks, pruning heavy mods) if the logs point somewhere clear, so look there first: read the logs for a mod misbehaving or errors repeating, and check the game's own settings for a view distance or entity cap set higher than anyone wanted. If you find a cause, fix that. If the server is genuinely doing more work than it was sized for rather than leaking, raise its memory limit with set_resources — it applies to the running server without restarting it or disconnecting anyone. Move in a sensible step, not to the ceiling, and say which of the two you concluded and what you did
