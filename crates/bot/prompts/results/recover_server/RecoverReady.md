---
id: RecoverReady
type: prompt
annotations:
  sent_when: >-
    tool result when recover_server brings an archived server back and it comes
    up healthy
  used_by:
    - file: discord/gary/tools.rs
      function: format_recover
  variables:
    name:
      source: the recovered server's name
      contents: the server name
    address:
      source: the recovered server's advertised address
      contents: the host:port players connect to
  reasoning:
    - >-
      The clean-success outcome: reports the address so the model can hand the
      user a working connection straight away.
---
recovered {{name}} — it's back up at {{address}}
