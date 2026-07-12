---
id: ReadyTimedOut
type: prompt
annotations:
  sent_when: >-
    tool result when a readiness wait times out — the server still isn't
    accepting players after a few minutes, without an outright crash
  used_by:
    - file: discord/gary/tools.rs
      function: format_ready_wait
  variables:
    server:
      source: the target server's instance name
      contents: the server name
  reasoning:
    - >-
      Offers a benign explanation (a big world can be slow to load) alongside the
      diagnostic option, so Gary neither declares failure prematurely nor ignores
      a possible problem — it's a wait-or-check, not a crash.
---
{{server}} still isn't accepting players after a few minutes — a big world can take a while to load, so check the logs or wait and try again
