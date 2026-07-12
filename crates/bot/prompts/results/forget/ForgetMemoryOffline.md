---
id: ForgetMemoryOffline
type: prompt
annotations:
  sent_when: >-
    tool result when forget can't reach long-term memory storage to remove a fact
  used_by:
    - file: discord/gary/tools.rs
      function: exec_forget
  reasoning:
    - >-
      Tells the model memory is unreachable so it can't change what's stored,
      distinct from a not-found id — the fact may still exist, it just can't be
      touched right now.
---
my long-term memory's offline right now, so I can't change it.
