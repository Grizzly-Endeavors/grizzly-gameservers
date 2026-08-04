---
id: AboutServerProcessUnknown
type: prompt
annotations:
  sent_when: >-
    appended to an about_server result when the supervisor's process state
    couldn't be read — the pod isn't up, or its control API didn't answer
  used_by:
    - file: discord/gary/tools.rs
      function: process_line
  reasoning:
    - >-
      Stated rather than omitted, for the same reason an unknown player count is:
      silence reads as "nothing to report", and the model would go on to describe
      a server as healthy on the strength of a reading it never got.
---
Process: uptime and crash count unavailable — the server's supervisor didn't answer, so nothing here says whether it has been crashing.
