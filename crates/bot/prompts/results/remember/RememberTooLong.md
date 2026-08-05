---
id: RememberTooLong
type: prompt
annotations:
  sent_when: >-
    tool result when remember is given a note longer than the store's cap, so
    nothing was saved
  used_by:
    - file: discord/gary/tools.rs
      function: exec_remember
  variables:
    max:
      source: the store's MAX_NOTE_CHARS cap, rendered by exec_remember
      contents: the maximum number of characters a note may be
  reasoning:
    - >-
      Says the note was not saved, so the model doesn't carry on as though the
      fact is now durable and skip telling anyone.
    - >-
      Names the limit and what a note is *for*, so the retry is a shorter real
      note rather than the same text split across several saves — which would
      defeat the cap it just hit.
---
that note wasn't saved: it is longer than {{max}} characters. a saved fact should be one or two sentences of durable operational knowledge about a game — not a transcript, a log excerpt, or a list of instructions. shorten it to the fact itself and try again.
