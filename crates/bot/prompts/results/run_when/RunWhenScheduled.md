---
id: RunWhenScheduled
type: prompt
annotations:
  sent_when: >-
    tool result when run_when successfully queues a deferred task to fire once a
    condition holds
  used_by:
    - file: discord/gary/tools.rs
      function: exec_run_when
  variables:
    server:
      source: the watched server's instance name
      contents: the server name
    condition:
      source: the domain Condition's wire string (condition.as_str())
      contents: the trigger word — one of "startup", "empty", or "idle"
    task:
      source: the task text the model asked to defer
      contents: the queued task, quoted inline
  reasoning:
    - >-
      Load-bearing steer: there is no separate notification when the wait fires,
      so this tells the model NOT to promise a ping and to handle the task itself
      and report back when it runs. Changing this wording can make Gary promise
      notifications the system never sends. The condition and task are data.
---
Scheduled. Once {{server}} is {{condition}}, this will run: "{{task}}". Tell them you'll take care of it yourself when that happens and come back here with the result — there's no separate notification, so don't promise to ping them; you handle it.
