---
id: WatchdogUnhealthy
type: prompt
annotations:
  sent_when: >-
    fills the WatchdogIncident trigger-note slot when the Agones GameServer is
    in the Unhealthy state — the supervisor's crash escalation stopped the
    health heartbeat and the cluster tore the pod down; the server stays down
    until an admin recovers it
  used_by:
    - file: watchdog/respond.rs
      function: trigger_note
  reasoning:
    - >-
      The bleakest trigger note, and honest about it: the pod (and with it the
      supervisor, the live logs, and every tool that reaches them) is gone, so
      the model must not burn its rounds retrying status/logs calls that can
      only fail. What it can still do is reason from what it knows — the game,
      recent changes it remembers making, its memories — and report; saying
      "recovery is an admin action" out loud is what licenses a manager-tier
      turn to stop at a clear report instead of experimenting.
    - >-
      Reassures that the world's data survives on its volume so the summary
      Gary posts doesn't accidentally read as "the world is lost" — friends
      reading the channel should learn the server is down, not panic about
      their builds.
---
has crashed repeatedly and been condemned by the cluster — the supervisor gave up, the health check failed, and the server's pod has been torn down entirely. The world's data is safe on its volume, but the server will stay down until it is recovered, which is an admin action you cannot perform. Your usual tools (status, logs, restart) reach the pod that no longer exists, so don't retry them against this server. Piece together the likely cause from what you know — the game, any recent config changes, your memories — say plainly what happened and what you suspect, and flag that bringing it back needs Bear (or an admin running /recover)
