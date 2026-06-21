## ADDED Requirements

### Requirement: One canonical `confirm` verb for two-step destructive commands
Every two-step DESTRUCTIVE operator command (currently `wipe-workspace` AND `rollback`; any future two-step destructive op inherits this) SHALL be confirmed by ONE canonical second-step verb, `confirm`. The dispatcher SHALL accept BOTH `@<bot> confirm` AND the bare token `confirm` (no mention) as that second step;
`@<bot> confirm` is the documented canonical form. No per-op confirm verb
(`wipe-workspace-confirm`, `rollback-confirm`) appears in the documented
interface.

`confirm` SHALL be channel-keyed: a channel SHALL hold AT MOST ONE pending
destructive op at a time, recorded as a TAGGED value identifying which op is
pending (pending-wipe, pending-rollback, or a future variant). A destructive
command's first-step preview SHALL RECORD that op as the channel's pending op
with a 60-second TTL, REPLACING any op already pending on that channel. On
`confirm`, the dispatcher SHALL consume the channel's pending op AND submit the
control-socket action for whichever destructive op it is.

When `confirm` arrives AND the channel has no live pending op — none recorded, OR
the recorded op's 60-second TTL has expired — the dispatcher SHALL reply with a
single clear message stating there is no pending confirmation in this channel
(or it expired) AND to re-issue the original command; it SHALL submit no action.

The dispatcher SHALL keep `wipe-workspace-confirm` AND `rollback-confirm` as
DEPRECATED ALIASES that still execute the channel's pending op, so an operator
mid-flow is not broken. The deprecated aliases SHALL NOT appear in the `help`
verb list AND SHALL NOT be named by any first-step preview message. (Their
removal MAY follow in a later change.)

#### Scenario: `confirm` resolves a pending wipe
- **WHEN** an operator posts `@<bot> wipe-workspace myrepo` (recording a pending
  wipe for the channel) AND then posts `@<bot> confirm` within the 60s TTL
- **THEN** the dispatcher consumes the channel's pending op AND submits the
  `wipe_workspace` action for the captured repo
- **AND** no `rollback_recovery` action is submitted

#### Scenario: `confirm` resolves a pending rollback
- **WHEN** an operator posts `@<bot> rollback myrepo 3` (recording a pending
  rollback for the channel after the dry-run preview) AND then posts
  `@<bot> confirm` within the 60s TTL
- **THEN** the dispatcher consumes the channel's pending op AND submits the
  confirmed (non-dry-run) `rollback_recovery` action for the captured repo AND
  depth
- **AND** no `wipe_workspace` action is submitted

#### Scenario: Both bare and mentioned forms are accepted
- **WHEN** a destructive op is pending on a channel AND the operator replies with
  the bare token `confirm` (no mention)
- **THEN** the dispatcher consumes the pending op AND executes it, identically to
  `@<bot> confirm`

#### Scenario: `confirm` with no pending op errors clearly
- **WHEN** an operator posts `confirm` (bare OR mentioned) on a channel that has
  no recorded pending destructive op
- **THEN** the dispatcher replies that there is no pending confirmation in this
  channel (or it expired — re-issue the original command) AND submits no action

#### Scenario: `confirm` after the TTL expired errors clearly
- **WHEN** a destructive op was previewed on a channel but more than 60 seconds
  elapse before `confirm` arrives
- **THEN** the expired pending op is not executed; the dispatcher replies with the
  same no-pending-confirmation message AND submits no action

#### Scenario: A new destructive preview replaces a prior pending in the same channel
- **WHEN** an operator previews one destructive op (e.g. `wipe-workspace`) AND
  then, before confirming, previews a second destructive op (e.g. `rollback`) in
  the same channel
- **THEN** the second op REPLACES the first as the channel's single pending op
- **AND** a subsequent `confirm` executes the SECOND op only; the first op is no
  longer pending AND is not executed

#### Scenario: The deprecated per-op aliases still execute the pending op
- **WHEN** a destructive op is pending on a channel AND the operator replies with
  a deprecated alias (`@<bot> wipe-workspace-confirm` OR `@<bot> rollback-confirm`)
- **THEN** the dispatcher executes the channel's pending op (the alias routes to
  the same channel-keyed confirm path; the channel's pending entry is
  authoritative)
- **AND** neither deprecated alias appears in the `help` verb list NOR in any
  first-step preview message

## MODIFIED Requirements

### Requirement: Wipe-workspace confirmation shows live repository context
The first-step warning message for `@<bot> wipe-workspace <repo>` SHALL include a context preview drawn from the same live data the per-repo `status` command surfaces. The preview names the workspace path being deleted, the currently-busy state (`idle` or `working on <change> (started <age> ago) — will be cancelled`), a one-line queue summary, and any active git-tracked operator markers that would persist across the wipe. Sections collapse when their underlying data is empty (no marker section when no markers exist; queue clause collapses to `empty queue` when all categories are zero). The trailing line SHALL instruct the operator to reply with the canonical confirmation verb `@<bot> confirm` within 60 seconds to proceed; it SHALL NOT name the deprecated per-op verb `wipe-workspace-confirm`.

#### Scenario: Confirmation message names the in-flight change when busy
- **WHEN** an operator posts `@<bot> wipe-workspace myrepo` AND the daemon is currently working on change `audit-proposal-self-validation` (busy marker present, started 5 minutes ago)
- **THEN** the first-step warning text contains `Currently: working on \`audit-proposal-self-validation\` (started 5m ago) — will be cancelled`
- **AND** the warning text contains the workspace path being deleted
- **AND** the warning text contains the queue clause

#### Scenario: Confirmation message reads `idle` when no iteration is in flight
- **WHEN** an operator posts `@<bot> wipe-workspace myrepo` AND no busy marker exists for the repo
- **THEN** the warning text contains `Currently: idle`
- **AND** the warning text does NOT contain a `— will be cancelled` clause

#### Scenario: Active markers section appears only when markers exist
- **WHEN** the repo has at least one `.perma-stuck.json` OR `.needs-spec-revision.json` marker file under any active or excluded change
- **THEN** the warning text contains an `Active markers (git-tracked; preserved across the wipe):` section listing each marker as `• <change> (<marker-file>)`
- **WHEN** the repo has no such markers
- **THEN** the warning text does NOT contain the active-markers section at all (no empty section, no `(none)` placeholder)

#### Scenario: Queue clause collapses to `empty queue` when all categories are zero
- **WHEN** the repo's pending, waiting, and excluded queue categories are all empty
- **THEN** the warning text's queue line reads `Queue (continues after wipe): empty queue`

#### Scenario: User-controlled fields are Slack-escaped
- **WHEN** a change name appearing in the queue clause OR the markers section contains a `<` character (despite the parser's allowlist; belt-and-braces)
- **THEN** the rendered warning text contains `&lt;` in place of the literal `<`

#### Scenario: The confirmation prompt names the canonical confirm verb
- **WHEN** an operator posts `@<bot> wipe-workspace myrepo`
- **THEN** the trailing prompt line instructs the operator to reply `@<bot> confirm` within 60 seconds
- **AND** the warning text does NOT name the deprecated `wipe-workspace-confirm` verb

### Requirement: Help verb returns the verb list
The dispatcher SHALL recognize `@<bot> help` (case-insensitive) as a verb and return `Some(Reply::Sync(text))` where `text` enumerates every currently-supported verb, its syntax, and a one-line description, plus a one-line pointer to the README's confirmation-flow section for the destructive verbs. The verb list SHALL include `confirm` as the SINGLE second-step verb for the two-step destructive commands (`wipe-workspace`, `rollback`), described as applying to all of them; the deprecated per-op aliases `wipe-workspace-confirm` AND `rollback-confirm` SHALL NOT appear in the list, AND the `wipe-workspace` AND `rollback` descriptions SHALL say each awaits `@<bot> confirm` rather than an op-specific verb.

#### Scenario: help returns a multi-line synopsis
- **WHEN** `handle_message("@<bot> help", ...)` is called
- **THEN** the return value is `Some(Reply::Sync(text))`
- **AND** `text` contains the strings `status`, `clear-perma-stuck`, `clear-revision`, `ignore-and-continue`, `clear-ignore`, `wipe-workspace`, `rebuild-specs`, AND `help` (the current verb set)

#### Scenario: help is case-insensitive
- **WHEN** `handle_message("@<bot> HELP", ...)` is called
- **THEN** the return value is `Some(Reply::Sync(text))` matching the lowercase form

#### Scenario: help lists a single confirm verb and hides the deprecated aliases
- **WHEN** `handle_message("@<bot> help", ...)` is called
- **THEN** `text` lists `confirm` as the second-step verb for the two-step destructive commands
- **AND** `text` does NOT contain `wipe-workspace-confirm` NOR `rollback-confirm`
