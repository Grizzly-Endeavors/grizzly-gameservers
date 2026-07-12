---
id: FsUnreachable
type: prompt
annotations:
  sent_when: >-
    tool result when a filesystem op can't reach the server at all
  used_by:
    - file: discord/gary/tools.rs
      function: fs_result
  variables:
    server:
      source: the target server's instance name
      contents: the server name
  reasoning:
    - >-
      The reach-failure counterpart to the not-ready file result; both transient,
      but this one couldn't contact the server, worded "try again in a moment".
---
I couldn't reach {{server}} just now — worth trying again in a moment
