---
id: AboutServerLastAction
type: prompt
annotations:
  sent_when: >-
    appended to an about_server result when the event log holds a lifecycle
    action for this server
  used_by:
    - file: discord/gary/tools.rs
      function: history_lines
  variables:
    action:
      source: the kind of the newest lifecycle event for this server
      contents: >-
        what was done, in past tense, like "started", "restarted", "stopped" or
        "restored"
    actor:
      source: the actor label on that event
      contents: who or what did it — a person, Gary, or an automatic cycle
    when:
      source: the created_at of that event
      contents: the timestamp of the action
  reasoning:
    - >-
      Turns "the server is stopped" into "someone stopped it twenty minutes ago",
      which is the difference between Gary restarting something a person had
      deliberately paused and Gary noticing that it fell over.
---
Last change: {{action}} by {{actor}}, {{when}}.
