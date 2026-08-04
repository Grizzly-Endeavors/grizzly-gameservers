---
id: AboutServerProcess
type: prompt
annotations:
  sent_when: >-
    appended to an about_server result when the server's supervisor reported its
    process state
  used_by:
    - file: discord/gary/tools.rs
      function: process_line
  variables:
    uptime:
      source: uptime_seconds from the supervisor's status route, rendered as a duration
      contents: >-
        how long the current game process has been running, like "3d 4h" or
        "12m"; "just started" when it is under a minute old
    restarts:
      source: the restarts counter from the supervisor's status route
      contents: >-
        the number of unexpected exits the supervisor has relaunched from since
        the pod started, as a number
  reasoning:
    - >-
      Crash count is the one health signal nothing else surfaces: a server that
      quietly relaunches every few minutes looks "Ready" in every listing. Pairing
      it with uptime is what makes it readable — five crashes over a month is
      noise, five in an hour is a broken mod.
    - >-
      Says "since the pod started" because the counter resets when Agones
      reschedules the pod, and a model reading it as lifetime history would
      reassure someone about a server that has in fact been crash-looping.
---
Process: up {{uptime}}, with {{restarts}} crash restart(s) since the pod started.
