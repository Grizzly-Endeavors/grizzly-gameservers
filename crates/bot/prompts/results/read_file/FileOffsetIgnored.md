---
id: FileOffsetIgnored
type: prompt
annotations:
  sent_when: >-
    a read_file asked for an offset but the server's supervisor answered from
    the start of the file, meaning it predates paged reads
  used_by:
    - file: discord/gary/tools.rs
      function: format_file
  reasoning:
    - >-
      A server keeps the supervisor baked into its running image until it next
      starts, so an offset can be silently ignored by an older one. Saying so is
      the difference between the model knowing it is looking at the wrong part
      of the file and it confidently reporting on the wrong part of the file.
    - >-
      Names the shell as the way through, because that is the capability that
      still reaches the middle of a file on an older server.
---
(this server is running an older supervisor that ignored the offset, so this is the start of the file — use run_shell with sed or tail to reach further in)
