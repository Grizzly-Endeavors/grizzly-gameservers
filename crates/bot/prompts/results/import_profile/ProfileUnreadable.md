---
id: ProfileUnreadable
type: prompt
annotations:
  sent_when: >-
    tool result when a profile payload was fetched but could not be read as an
    r2modman export
  used_by:
    - file: discord/gary/mod_list.rs
      function: exec_import_profile
  variables:
    code:
      source: the profile code the model passed to the tool
      contents: the code, quoted inline
    reason:
      source: the codec's own message for why the payload was rejected
      contents: >-
        a lowercase phrase naming what was wrong with the payload — a missing
        marker, bad base64, a missing export.r2x, or a size or package-count
        limit
  reasoning:
    - >-
      Third-party bytes that didn't parse, so the reason is included verbatim
      rather than flattened to "invalid": the two cases behind it are very
      different to a user — a code pointing at something that isn't a profile,
      and a profile too large for this platform to accept.
    - >-
      Says nothing was written, because the natural fear after a failed import
      is that the mod list was left half-changed.
---
The profile behind {{code}} couldn't be read: {{reason}}. Nothing on the server has been changed. If they exported it from something other than r2modman or Thunderstore Mod Manager, that's likely why; otherwise ask them to export it again.
