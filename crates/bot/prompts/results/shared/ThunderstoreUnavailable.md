---
id: ThunderstoreUnavailable
type: prompt
annotations:
  sent_when: >-
    tool result when a search_mods or mod_info call for a Thunderstore game
    can't reach the registry, or gets an error or unparseable reply back from it
  used_by:
    - file: discord/gary/tools.rs
      function: thunderstore_unavailable
  reasoning:
    - >-
      Keeps an outage from reading as an answer: without this the model could
      take a failed lookup for "no such mod" and either give up on a mod that
      exists or install an identifier it never actually verified.
    - >-
      Says to wait rather than to proceed, because an unverified entry written
      now is one that stops the server at its next restart — and the version to
      pin only exists in the reply that didn't arrive.
---
couldn't reach the mod registry just now — this doesn't mean the mod is missing, so don't rule anything out or install anything on the strength of it. Say the registry is temporarily unreachable and offer to try again shortly.
