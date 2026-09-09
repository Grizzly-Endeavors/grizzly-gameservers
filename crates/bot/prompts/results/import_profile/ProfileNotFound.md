---
id: ProfileNotFound
type: prompt
annotations:
  sent_when: >-
    tool result when import_profile reaches the registry successfully but it has
    no profile behind that code
  used_by:
    - file: discord/gary/profiles.rs
      function: exec_import_profile
  variables:
    code:
      source: the profile code the model passed to the tool
      contents: the unresolved code, quoted inline
  reasoning:
    - >-
      An ordinary answer rather than a failure — a code is a UUID typed or
      pasted by hand, and a wrong character is the likeliest cause by a wide
      margin, so the fix is to check the code rather than to conclude the
      registry is broken.
    - >-
      Says codes don't expire, because the natural guess for "it isn't there" is
      that it aged out, and someone who believes that stops re-checking what
      they typed.
---
Thunderstore has no profile behind the code {{code}}. Codes don't expire, so this is almost certainly a typo or a partly-copied code — ask them to paste it again straight out of their mod manager. Nothing on the server has been touched.
