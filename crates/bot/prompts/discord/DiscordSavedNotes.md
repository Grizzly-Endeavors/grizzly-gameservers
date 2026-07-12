---
id: DiscordSavedNotes
type: prompt
annotations:
  sent_when: >-
    appended for managers and admins (access >= Manager) only when Gary has
    saved notes for the relevant games — the rendered notes block is non-empty
  used_by:
    - file: discord/gary/mod.rs
      function: append_manager_guidance
  variables:
    memories:
      source: >-
        render_memories in memory.rs (via GaryMemory::render_for_prompt), which
        groups the saved facts by scope.
      contents: >-
        the rendered saved-notes block — scope headers each followed by their
        "  - #<id>: <fact>" lines. Never empty here: when it renders empty this
        whole block is omitted in code, so the framing never appears alone.
  reasoning:
    - >-
      Frames the injected notes so Gary treats them as his own durable memory and
      knows he can forget one by its # if it's wrong. The notes themselves are
      computed data and enter through the variable; only this one-line framing is
      prompt text. The framing ends with a colon-newline so the first scope
      header sits directly beneath it, matching render_memories' output shape.
---
Things you've learned about these games (durable notes you saved; forget one by its # if it's wrong):
{{memories}}
