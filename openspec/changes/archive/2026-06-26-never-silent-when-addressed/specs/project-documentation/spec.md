## ADDED Requirements

### Requirement: The bot acknowledges every request addressed to it, never silently dropping it

When an operator authorized on that surface **addresses** the bot — a `@<bot>` command in chatops (where channel write-access is the authorization boundary), a `@<bot>` comment from an authorized commenter on a forge pull request (GitHub, and any future GitLab/other forge), or any future addressed surface — AND the bot has **no handler** for the request (an unrecognized verb, a malformed command, a request it cannot map to any action), it SHALL emit a non-silent acknowledgment rather than dropping the request without any signal. The acknowledgment scales to the surface and the need: a reaction where a minimal "not understood" signal suffices (the chatops `?` reaction), OR a reply naming what the bot can do where the affordance is more useful (a forge PR comment).

A message is **addressed** when the bot mention is the leading token of a command attempt (the "first token is `@<bot>`" rule the existing triggers use). A message that does NOT address the bot — an incidental mention (`cc @<bot>`), or ordinary channel/PR conversation with no mention — is NOT subject to this requirement; the bot SHALL NOT react to ordinary conversation. **Silent** means no acknowledgment of any kind; a `?` reaction is an acknowledgment, not silence.

This standard governs requests the bot has **no handler** for. It does NOT govern a RECOGNIZED request the bot deliberately rate-limits, caps, or declines under policy (e.g. a per-PR revision cap): those follow their own one-time-decline-then-deduplicate policy, where a single decline is the acknowledgment AND subsequent duplicates of the same request MAY be deduplicated without re-acknowledging. It ALSO does NOT govern a request dropped by an access-control gate before the bot considers acting — e.g. a commenter not authorized on that surface — which follows that gate's own decline-or-stay-silent policy (such as the forge `decline_comment` flag, default no-reply to avoid public-PR spam and feedback loops). "Never silent" requires at least one acknowledgment per distinct addressed request **from an authorized operator** that the bot cannot handle — not a reply to every repeat of an already-answered one, AND not a reply to a request an access-control gate has already dropped.

This is a cross-surface UX invariant, sibling to `Control-plane gatekeepers fail closed`: a control that silently does nothing when addressed leaves the operator unable to tell whether the bot saw the request, is working, or rejected it.

#### Scenario: An unrecognized addressed request is acknowledged, not dropped
- **WHEN** an operator authorized on that surface addresses the bot (mention as the leading token) AND the request matches no handler the bot can act on
- **THEN** the bot emits a non-silent acknowledgment — a reaction where a minimal signal suffices, OR a reply naming what it can do where the affordance is more useful
- **AND** the operator is never left with no signal at all

#### Scenario: A non-addressing message is not reacted to
- **WHEN** a message does not address the bot (an incidental mention, or ordinary conversation with no mention)
- **THEN** the bot does NOT react or reply
- **AND** ordinary conversation is never decorated with acknowledgments

#### Scenario: An access-control-gated request follows the gate's policy, not this standard
- **WHEN** a commenter who is NOT authorized on the surface addresses the bot with a request the bot has no handler for (e.g. an unauthorized author on a public forge PR)
- **THEN** the request follows that surface's access-control policy (e.g. the forge `decline_comment` flag — by default no reply, to avoid public-PR spam and feedback loops)
- **AND** this standard does NOT compel an acknowledgment

#### Scenario: A reaction counts as acknowledgment
- **WHEN** the surface supports reactions AND the bot acknowledges an unrecognized addressed request with the `?` reaction
- **THEN** the invariant is satisfied without a text reply
- **AND** "silent" is reserved for the case of no acknowledgment of any kind
