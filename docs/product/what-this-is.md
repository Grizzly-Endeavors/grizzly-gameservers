# Grizzly Gameservers — what it is

Plain-language description. No technical detail — just the shape.

---

## The problem

A group of friends wants to play a game together. Somebody has to run the server.

That person becomes permanent tech support. *It's down. Can you add this mod? Can you turn off
PvP? Why did it crash? Can you roll back to yesterday?* Every question routes through one person, and
that person has to be awake, available, and willing.

The existing options don't fix it, they relocate it:

- **A control panel** hands the job to whoever is most technical and assumes they know what a
  config file is. Non-technical friends can't use it, so the bottleneck stays.
- **A support ticket** takes hours and requires explaining the problem to someone who can't see the
  server.
- **Free hosting** makes you wait in a queue to play.

The bottleneck is never the hardware. It's that running a game server requires knowing things
non-technical people have no reason to know.

---

## What it is

**A Discord bot that runs game servers for a friend group, and an AI operator named Gary who does the
actual administration.**

Two halves:

**The bot** handles the mechanics. Type a command in Discord, get back an address to connect to.
Create a server, start it, stop it, back it up, throw it away. No account to make, no panel to log
into, no software to install.

**Gary** does the part that used to need a person. You talk to him in plain English, in the same
channel where you were already talking:

> *"The server feels laggy."*
> *"Can we add this mod?"*
> *"Turn the difficulty down."*
> *"Something broke after the last change."*

He looks at how the server is actually configured, makes the change, restarts it, checks that it
worked — and puts it back the way it was if it didn't. He'll also do things later on request:
*"restart it once everyone logs off"* is a thing you can just say.

You can talk to him from inside the game too, without alt-tabbing.

---

## Who it's for

Non-technical friend groups. Specifically two people:

- **The one who'd otherwise be the admin** — gets their evenings back.
- **Everyone who'd otherwise ping that person** — can just ask directly.

Not for people who want to touch infrastructure. The whole point is that the machinery stays
invisible.

---

## What makes it different

**Everyone else gives you a control panel. This gives you an operator.**

That's the entire distinction, and it's bigger than it sounds. A panel makes *you* the administrator
— it just gives you nicer buttons for the job. Gary *is* the administrator. Nobody in the group needs
to learn what a server config is, because nobody in the group has to touch one.

Three things follow from that:

**It lives where the group already is.** Not a website someone logs into alone — a participant in the
channel where they already talk. Everyone sees it work. Nobody holds keys the others don't.

**It gets better at each game over time.** Every game stores its settings differently and every game
has quirks. Gary writes down what he learns about a game and carries it forward — so a problem solved
once for one group is already solved for the next one.

**It's built so it can't make things worse.** It saves the current state before changing anything,
checks whether the change actually worked, and undoes it automatically if the server doesn't come
back healthy. There are things it simply cannot do — deleting a world, changing who has access — no
matter who asks. When it isn't confident, it stops and says so rather than guessing.

That last part is what makes "ping the bot instead of a human" a real offer rather than a slogan.

---

## Where it is now

Live and running. Real friend groups, real servers, six games — Minecraft, Valheim, Palworld,
Factorio, Terraria, Satisfactory. Adding a new game is a small, repeatable job rather than a rebuild.

Built as a private service on personal hardware, which is the current limit — not the design.

---

## The one-liner

**Game servers your friends can run without anyone becoming the admin.**

Or, closer to what's actually novel:

**An AI that operates your game server — reads the config, makes the change, verifies it, and undoes
it if it broke something. Ask it in Discord like you'd ask a person.**
