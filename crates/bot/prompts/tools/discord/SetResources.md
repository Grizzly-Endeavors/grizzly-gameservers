---
id: SetResources
type: tool
tool_schema:
  name:
    type: string
    description: Exact server name to resize, as shown by `list_servers`.
  cpu_request:
    type: string
    description: >-
      CPU the server is guaranteed, e.g. "500m" for half a core or "2" for two
      cores. This is what it always has available; it is what gets reserved on
      the machine.
  cpu_limit:
    type: string
    description: >-
      Most CPU the server may use when it needs it, in the same form. Must be at
      least the request.
  memory_request:
    type: string
    description: >-
      Memory the server is guaranteed, e.g. "2Gi" or "512Mi". This is what gets
      reserved.
  memory_limit:
    type: string
    description: >-
      Most memory the server may use, in the same form. Must be at least the
      request. Going over this is what gets a server killed for running out of
      memory, so it is usually the figure to raise.
annotations:
  sent_when: >-
    offered on the Discord surface to managers and admins, and so to the
    proactive watchdog's unattended turns, which run at manager tier
  used_by:
    - file: discord/gary/tools.rs
      function: tool_table
    - file: discord/gary/tools.rs
      function: dispatch_mutating
    - file: discord/gary/tools.rs
      function: exec_set_resources
  reasoning:
    - >-
      Manager-tier so the watchdog can act on the memory-pressure incident it
      exists to detect instead of only reporting it. What bounds runaway
      allocation is the clamp in crate::sizing, not the tier — so the body's job
      is to make raising the numbers the *last* option rather than the first.
    - >-
      The "look for the cause first" paragraph is the load-bearing half. Growth
      is the easy answer and usually the wrong one: a memory leak, a runaway
      mod, or a view distance nobody meant to set will consume whatever it is
      given, and resizing hides it until the bigger limit fills too. Keep the
      concrete alternatives if reworded — a model told only "be careful" is not
      told what to do instead.
    - >-
      All four figures are required rather than optional. A partial change
      leaves a request from before paired with a limit from now and can invert
      the two, which Kubernetes rejects as an unschedulable pod — so the model
      is made to state the whole shape, having read it from server_resources.
    - >-
      Quantities are strings in Kubernetes' own notation, not numbers, because
      "2Gi" and "500m" are what every other surface (the catalog, kubectl,
      server_resources' own output) shows. Parsing happens at the dispatch
      boundary and a bad value is refused with the reason.
    - >-
      The body states the no-restart property plainly. It is the reason this
      tool exists rather than a stop/start pair, and a model that doesn't know
      it will reach for run_when and defer a change that never needed deferring.
    - >-
      The lowering-memory asymmetry is called out because it is the one case
      where the headline promise doesn't hold: Kubernetes forbids shrinking a
      memory limit in place unless the container restarts on resize, and the
      catalog deliberately doesn't ask for that. Without this the model reports
      a reduction as done when it is only scheduled.
    - >-
      The JVM caveat is in the body because it is only wrong on some games and
      only for memory: raising the pod's memory on Minecraft does not raise the
      Java heap, which is set at launch from the overlay.
---
Change how much CPU and memory a server is allowed. This takes effect on a running server immediately, without restarting it and without disconnecting anyone playing — so you don't need to wait for the server to be empty, and shouldn't schedule it for later. Call server_resources first and state all four figures, since a request above its own limit is refused.

Look for the cause before reaching for more. A server pressed against its limits usually has a reason you can find and fix more cheaply: check read_logs for a mod misbehaving or errors repeating, and the game's own settings for a view distance, entity cap, or world-border that was set higher than anyone wanted. Growing the allocation is the right answer when the server is genuinely doing more work than it was sized for — more players, more world, a heavier modpack — and the wrong answer when something is leaking or looping, because that will fill whatever you give it and you'll be back with a bigger number to explain. Say which of the two you concluded and why.

When you do resize: raising the memory limit is the fix for a server being killed for running out of memory, and raising the CPU limit is the fix for one that stutters under load. Move in a sensible step rather than to the ceiling — enough headroom to hold, not everything available. Lowering is worth doing for a server sitting far under what it reserves, with one asymmetry: raising anything and lowering CPU both happen live, but **lowering a memory limit never does** — that one is saved and takes effect on the next restart, because applying it live would mean restarting the server, which is the thing this avoids. So say when a reduction will land rather than implying it already has. On Java games like Minecraft the pod's memory and the game's own heap are separate: raising this gives the server more room overall and stops it being killed, but the heap itself is set when the game starts and changing that needs a restart.
