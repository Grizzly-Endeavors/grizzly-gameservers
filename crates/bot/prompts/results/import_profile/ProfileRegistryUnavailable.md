---
id: ProfileRegistryUnavailable
type: prompt
annotations:
  sent_when: >-
    tool result when import_profile could not reach the registry to work out
    where each mod in the profile has to be installed
  used_by:
    - file: discord/gary/profiles.rs
      function: exec_import_profile
  reasoning:
    - >-
      The whole import is refused rather than written from unclassified entries,
      and the result says so plainly: retrying is cheap and changes nothing,
      while a mod list written on guesses is durable, silently wrong, and looks
      like it worked.
    - >-
      Explicitly a "try again", not a "this profile is bad" — the difference
      decides whether the user goes looking for a new code they don't need.
---
Couldn't reach the mod registry to work out which of that profile's mods belong on the server and which belong on players' machines, so nothing was written. This is the registry being unreachable, not a problem with the code — offer to try the same code again in a minute.
