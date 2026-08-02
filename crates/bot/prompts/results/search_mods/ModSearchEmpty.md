---
id: ModSearchEmpty
type: prompt
annotations:
  sent_when: >-
    tool result when search_mods reaches Modrinth successfully but the query and
    its filters match nothing
  used_by:
    - file: discord/gary/tools.rs
      function: format_mod_search
  variables:
    query:
      source: the search text the model passed to the tool
      contents: the query text, quoted inline
  reasoning:
    - >-
      Distinguishes an empty result from a failed call so the model retries
      instead of reporting the registry down. It names the filters as the likely
      cause because they are: a Minecraft version or loader filter that no
      project matches produces exactly this, and dropping one is the fix.
---
nothing on Modrinth matched {{query}} — a filter may be too narrow (an unsupported Minecraft version or loader rules everything out), so try dropping one or searching different words.
