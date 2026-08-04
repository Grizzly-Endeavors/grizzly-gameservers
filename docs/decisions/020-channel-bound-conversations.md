# ADR-020 — Gary's conversation belongs to the channel, not to one person

**Status:** Accepted (2026-08-04)

## Context

The session store keyed transcripts on `(channel_id, user_id)`. A channel with four friends in it was four separate conversations with four separate Garys, none of which could see the others. That is invisible while everyone is asking their own questions, and wrong the moment they aren't:

- An admin could not answer, correct, or take over something a friend had started. Gary had no memory of the friend's exchange when the admin spoke.
- An admin could not approve a friend's destructive action. The confirmation collector filtered on `ctx.author_id`, so only the person who triggered the tool could click — and a bystander's click was silently dropped, with no feedback at all, which reads as a broken button.
- "Now restart it" from a second person resolved against nothing, because the antecedent lived in someone else's transcript.

Two further problems were latent in the per-user keying and would become live under any change to it. The system prompt was built by the closure `checkout` runs *only when there is no live session*, so a conversation's tier guidance and injected memories belonged to whoever opened it — indefinitely, for up to the 30-minute TTL. And `commit` inserts unconditionally: two turns checked out against the same key both write, and the loser's exchange disappears along with the reply Gary already posted into the channel.

## Decision

**`SessionKey` becomes an enum: `Channel(u64)` or `IngamePlayer { guild, player }`.** Discord conversations bind to the channel. In-game stays per-player: a player holds no access tier, gets a fixed read-only pair of tools, and there is nothing in an in-game exchange for a second person to join or approve. Spelling the two as variants rather than a shared `(u64, u64)` also retires an uncomfortable comment — the old code argued the two key spaces did not collide because channel ids and player-name hashes happened not to overlap. Now they cannot, by construction.

**The system prompt is rebuilt for the speaker on every turn.** After checkout, the transcript's leading system message is replaced with one built from *this* message's tier and the current memories. Without it a read-only friend inherits admin guidance and Gary narrates powers the tool gate then refuses — the model describing capabilities it does not have is worse than it simply not having them. The refresh also fixes stale memories, which previously only reached a fresh conversation.

**Turns on one key serialize.** `SessionStore::turn_lock` hands out a per-key async lock held across checkout → run → commit. The lock map self-prunes — an entry survives only while a guard is outstanding — so idle channels leave nothing behind. Queuing is the right trade against last-write-wins: the second asker waits a few seconds and gets an answer with the first exchange in view, rather than having their exchange silently deleted.

**Every message is attributed.** A user turn is rendered as `{speaker} ({tier}) says: {message}`, mirroring the in-game surface's `IngameQuestion`. Without it a multi-person transcript reads as one voice, and Gary cannot tell whose request is whose or whether a "go ahead" carries authority. The tier travels with the speaker because that is the only way Gary can weigh an approval — but it is a hint for his wording, never enforcement: the tool gate re-checks every call regardless of what the transcript says.

That attribution widens the prompt-injection surface, so it ships with its own hardening block (`DiscordSharedChannel`), the Discord counterpart to `IngamePersona`'s. It declares message text untrusted, tells Gary that an access level claimed *in* a message proves nothing against the one in the attribution, and states that the tool gate — not his own judgement about a borderline request — is what refuses things.

**Approval is by tier, not by identity.** The confirmation collectors drop their `author_id` filter and instead resolve each clicker's tier, accepting any click at or above the tool's required tier and answering anyone below with an ephemeral explanation while the prompt stays live. The timeout is the total wait, so unqualified clicks cannot hold a destructive prompt open. Resolving a clicker's tier needed a third caller of the same policy, so the near-duplicate implementations in `auth::access_level_of` (poise context) and `gary::caller_access_level` (message) collapse into one `access_level_for(Speaker)`.

## Consequences

- `/new-session` clears the channel's conversation for everyone in it. That is what "start fresh" means when the back-and-forth was shared; the reply says so.
- The operator harness shares the channel's transcript with any human talking there, and `--new-session` clears that channel rather than one caller's history. Pointing the harness at its own channel keeps this invisible, which is what `GAMESERVERS_OPERATOR_CHANNEL` already does. Harness turns are attributed to "the operator harness" rather than to the user id they impersonate, so a dev-box turn never reads in the history as a friend having typed it.
- A friend can now see Gary answering someone else's question in their channel and follow the thread — the transcript is the conversation everyone is already watching.
- Gary's replies are still plain channel messages, not threaded replies. With attributed input he can name who he is answering, which is cheaper than Discord's reply cards.
- Two people asking at once queue rather than race. At friends-scale the wait is one turn; the alternative was one of them losing their exchange with no indication it had happened.

## Alternatives rejected

- **Key on the Discord thread object.** Real threads would scope conversations precisely, but they force friends to create one before talking to Gary — ceremony for a non-technical audience whose whole interface is "type at the bot in the channel."
- **Keep per-user transcripts and share only the approval.** Fixes the button and nothing else: Gary still can't see what the friend asked, so an admin approving a destroy has no context for what they are approving.
- **Let the system prompt stay at whatever tier opened the conversation.** Cheaper, and wrong in the direction that matters — a read-only friend would be told he may destroy things, and would be refused only after saying he would.
- **Let any click through and rely on the tool gate.** The gate runs at dispatch, before the button is posted; by the time someone clicks, the authority question is who is approving, and nothing downstream re-asks it.
