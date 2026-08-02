---
id: ModrinthUnavailable
type: prompt
annotations:
  sent_when: >-
    tool result when search_mods or mod_info can't reach Modrinth, or gets an
    error or unparseable reply back from it
  used_by:
    - file: discord/gary/tools.rs
      function: modrinth_unavailable
  reasoning:
    - >-
      Keeps an outage from reading as an answer: without this the model could
      take a failed lookup for "no such mod" and either give up on a mod that
      exists or install a slug it never actually verified.
    - >-
      Says to wait rather than to proceed, because the same registry has to
      resolve the slug at the server's next boot — if it's unreachable now, an
      install made anyway is one that fails to start.
---
couldn't reach Modrinth just now — this doesn't mean the mod is missing, so don't rule anything out or install anything on the strength of it. Say the registry is temporarily unreachable and offer to try again shortly.
