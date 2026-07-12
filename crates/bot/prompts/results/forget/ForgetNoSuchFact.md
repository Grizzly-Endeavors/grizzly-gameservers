---
id: ForgetNoSuchFact
type: prompt
annotations:
  sent_when: >-
    tool result when forget is given an id that doesn't match any saved fact
  used_by:
    - file: discord/gary/tools.rs
      function: exec_forget
  variables:
    id:
      source: the fact id the model tried to forget
      contents: the id that wasn't found, as a number
  reasoning:
    - >-
      Steers the model to re-check the saved list rather than retrying a bad id,
      so it corrects the reference instead of insisting on one that doesn't exist.
---
I don't have a fact #{{id}} to forget — check the list of what I've saved
