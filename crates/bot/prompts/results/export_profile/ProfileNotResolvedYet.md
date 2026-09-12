---
id: ProfileNotResolvedYet
type: prompt
annotations:
  sent_when: >-
    tool result when export_profile found no record of what the server resolved,
    because it has not started since mods were set up
  used_by:
    - file: discord/gary/mod_list.rs
      function: exec_export_profile
  variables:
    server:
      source: the server name the model passed to the tool
      contents: the server name, quoted inline
  reasoning:
    - >-
      A timing answer, not a failure, and one with an obvious next step — the
      record is written at every launch, so starting the server once produces
      it. Reporting this as an error would send the model looking for a problem
      that a restart fixes.
    - >-
      Deliberately does not fall back to reading the mod list directly: an
      unpinned entry there has no version, and a profile without versions is
      useless to the launcher on the other end, which is the entire point of
      handing one over.
---
{{server}} has no record of what it resolved, so there's nothing to build a code from yet — that record is written every time the server starts. If mods were just set up, start or restart it once and ask again.
