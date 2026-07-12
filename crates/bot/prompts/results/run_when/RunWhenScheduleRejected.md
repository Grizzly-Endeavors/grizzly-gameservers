---
id: RunWhenScheduleRejected
type: prompt
annotations:
  sent_when: >-
    tool result when run_when tried to enqueue a task but the queue rejected it
    (an error at enqueue time, distinct from the queue being unavailable)
  used_by:
    - file: discord/gary/tools.rs
      function: exec_run_when
  reasoning:
    - >-
      Distinguishes a transient enqueue failure from a missing queue, and steers
      the model to offer doing the task now instead of assuming it was scheduled.
---
I couldn't schedule that just now — the task queue didn't accept it. Offer to try doing it now instead.
