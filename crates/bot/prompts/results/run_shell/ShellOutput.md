---
id: ShellOutput
type: prompt
annotations:
  sent_when: >-
    tool result carrying a sandboxed shell command's output back to the model
    after run_shell
  used_by:
    - file: discord/gary/tools.rs
      function: format_shell
  variables:
    server:
      source: the server the command ran on
      contents: the server name
    command:
      source: the command line the model asked for
      contents: the command as given
    status:
      source: >-
        how the command ended, rendered from ShellExited, ShellSignalled or
        ShellTimedOut in format_shell
      contents: >-
        the rendered status fragment; it is the platform's own account, never
        anything the command printed
    output:
      source: the command's captured stdout and stderr, joined in format_shell
      contents: >-
        the captured bytes as returned, lossily decoded as UTF-8 and scrubbed of
        fence markers; stderr is appended after stdout under its own heading when
        both are present, and the joining is data and stays in code
  reasoning:
    - >-
      Fenced for the same reason log output is: the command reads files whose
      contents nobody here wrote — a mod's name, a player's chat line sitting in
      a log, a config value a friend typed. A shell makes that reach wider than
      read_logs, so the fence matters more here, not less. crate::untrusted
      removes any output line that tries to write a marker.
    - >-
      The status sits outside the fence because it is the platform's own account
      of how the process ended, and a command whose output claimed a different
      exit code should not be able to blur that line.
---
`{{command}}` on {{server}} {{status}}:
-----BEGIN UNTRUSTED SHELL OUTPUT-----
{{output}}
-----END UNTRUSTED SHELL OUTPUT-----
