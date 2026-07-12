---
id: OccupancyKnown
type: prompt
annotations:
  sent_when: >-
    appended to a server_status result when the live player count could be read
    from the server console
  used_by:
    - file: discord/gary/tools.rs
      function: occupancy_line
  variables:
    count:
      source: the live player count from the supervisor's occupancy check
      contents: the number of players currently online, as a number
  reasoning:
    - >-
      The "Players online:" label steers the model to report occupancy; the count
      is data. The newline that separates this from the summary line is a
      code-owned separator, so it isn't part of the body.
---
Players online: {{count}}
