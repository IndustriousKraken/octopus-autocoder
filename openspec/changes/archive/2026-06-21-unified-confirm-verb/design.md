# Design — Unified confirm verb

## Current state (grounded in code)

All sites are in `autocoder/src/chatops/operator_commands.rs`.

- Parser, bare-`confirm` shortcut: `parse_command_outcome_in_thread`
  (`:625`). The unmentioned branch (`:640-647`) maps a bare `confirm` token to
  `OperatorCommand::WipeWorkspaceConfirm { repo_substring: None }` — wipe ONLY.
  Bare `rollback-confirm` is not recognized at all.
- Verb dispatcher: `match verb.to_ascii_lowercase().as_str()` (`:666`).
  - `"wipe-workspace-confirm" | "confirm"` arm (`:755`) → `WipeWorkspaceConfirm`.
  - `"rollback-confirm"` arm (`:1361`) → `RollbackConfirm`. There is no bare
    form and `confirm` does NOT map to rollback.
- Two pending stores on `OperatorCommandDispatcher` (`:2656`):
  - wipe: `pending: ConfirmationStore` (`:2657`); struct `ConfirmationStore`
    (`:2523`) holds `HashMap<channel, PendingConfirmation { repo_url, expires_at }>`
    (`:2514`). `record` (`:2534`), `take_valid` (`:2556`).
  - rollback: `rollback_pending: RollbackConfirmationStore` (`:2660`); struct
    (`:2588`) holds `HashMap<channel, PendingRollback { repo_url, depth, expires_at }>`
    (`:2577`). `record` (`:2593`), `take_valid` (`:2613`).
  - Both initialized in `new` (`:2725-2726`).
  - The `RollbackConfirmationStore` doc-comment (`:2584-2586`) states it is
    "Separate from `ConfirmationStore` so a pending wipe AND a pending rollback
    can coexist on the same channel without clobbering each other." This
    coexistence is exactly the property the unified design removes.
- Record sites in the dispatcher:
  - wipe: `self.pending.record(...)` in the `WipeWorkspace` arm (`:3326`).
  - rollback: `self.rollback_pending.record(...)` in the `Rollback` arm (`:3594`).
- Take/execute sites:
  - `WipeWorkspaceConfirm` arm (`:3388`) → `self.pending.take_valid` (`:3389`);
    on `None` returns "no pending wipe-workspace confirmation in this channel
    (or it expired …)" (`:3392`).
  - `RollbackConfirm` arm (`:3602`) → `self.rollback_pending.take_valid` (`:3603`);
    on `None` returns "no pending rollback confirmation in this channel (or it
    expired …)" (`:3606`).
- Preview renderers:
  - `format_rollback_confirmation` (`:2036`) ends with
    `Reply \`@<bot> rollback-confirm\` within 60s to perform this rollback.` (`:2058`).
  - `format_wipe_confirmation` (`:2084`) ends with
    `Reply 'confirm' within {WIPE_CONFIRM_TTL_SECS} seconds to proceed.` (`:2169`).
- Help list `format_help_reply` (`:2345`):
  - `confirm` line for wipe (`:2357`).
  - `rollback-confirm` line for rollback (`:2375`).
- TTL: both flows use `WIPE_CONFIRM_TTL_SECS = 60` (`:52`, `:3329`, `:3598`).

## Canonical governance (what to MODIFY)

The chatops two-step confirm behavior is governed in `chatops-manager/spec.md`:

- "Wipe-workspace confirmation shows live repository context" (`:789`) — its last
  sentence pins the wipe preview's trailing `Reply 'confirm' within 60 seconds`
  line.
- "Wipe-workspace drains the in-flight iteration before deleting" (`:817`) —
  opens "On `confirm`, …"; the `confirm` verb wording is consistent with the
  unified design and does not need a verb-name change, but the requirement is
  the canonical home for "what the confirm step does for wipe".
- "Help verb returns the verb list" (`:459`).
- "`defer` and `undefer` …" (`:1656`) — its prose already contrasts the
  single-ack verbs against the destructive `wipe-workspace`/`rollback`
  channel-keyed two-step confirm; the unified `confirm` verb is consistent with
  that contrast and that requirement is NOT modified.

The rollback chatops two-step confirm is governed generically in
`orchestrator-cli/spec.md` "Code-rollback recovery …" (`:8025`), whose prose says
the operation requires "a two-step confirm for the chatops verb" (`:8049`) but
does NOT name the verb. That requirement stays in orchestrator-cli; the
verb-naming is a chatops-manager concern.

There is no existing requirement that names the second-step verb as a unified
concern. So this change ADDS one requirement in `chatops-manager` that pins the
unified `confirm` verb, the channel-keyed single-pending behavior, and the
deprecated aliases; and MODIFIES the two existing chatops-manager requirements
whose text currently names an op-specific verb or relies on a per-op preview
line ("Wipe-workspace confirmation shows live repository context" and "Help verb
returns the verb list"). The wipe-drains requirement is left as-is (its
`On \`confirm\`` wording already matches the unified verb).

## Decision: one tagged store, not check-both-with-precedence

The spec states the BEHAVIOR (one effective pending per channel; `confirm`
executes it; a new preview replaces any prior pending). Two structures could
satisfy that:

1. **Consolidate** the two stores into ONE `ConfirmationStore` whose value is a
   tagged enum `PendingDestructiveOp { Wipe { repo_url }, Rollback { repo_url,
   depth } }` (extensible with future variants), keyed by channel with the same
   60s TTL. `record_*` helpers insert the appropriate variant (replacing any
   existing entry for the channel); `take_valid` returns the tagged op; the
   `confirm` handler matches the variant and dispatches the corresponding action.
2. **Check both** existing stores in the `confirm` handler with a defined
   precedence.

This change picks (1), the consolidated tagged store. Rationale:

- The spec's core invariant is "at most ONE pending destructive op per channel,
  and a new destructive preview REPLACES any prior pending." A single tagged
  slot per channel enforces that invariant structurally — recording a wipe over
  a pending rollback (or vice-versa) is a single `HashMap::insert` that
  overwrites. Two separate maps CANNOT represent "at most one"; they let a
  pending wipe and a pending rollback coexist (the current code comment at
  `:2584` deliberately preserves that), so a bare `confirm` is genuinely
  ambiguous and "replace" is impossible to express without cross-store
  bookkeeping.
- Check-both-with-precedence leaves the two-coexisting-pendings hazard in the
  data model and only resolves it at read time, which is a tie-break rule, not
  the spec's "one effective pending." It also requires a stable precedence that
  the spec does not (and should not) need.
- The consolidated store is the smaller surface long-term: a future two-step
  destructive op adds one enum variant and one `record_*` helper rather than a
  third store plus a third precedence tier.

`PendingConfirmation` and `PendingRollback` collapse into the variants of the
tagged enum. `RollbackConfirmationStore` is removed; the dispatcher holds ONE
`pending: ConfirmationStore`.

## Verb / parser shape

- Bare `confirm` (unmentioned branch) and `@<bot> confirm` both parse to a single
  channel-keyed confirm intent (e.g. `OperatorCommand::Confirm`), NOT a
  wipe-specific one. The handler reads the channel's tagged pending op and
  dispatches accordingly.
- `@<bot> wipe-workspace-confirm` and `@<bot> rollback-confirm` remain accepted as
  DEPRECATED ALIASES routing to the same channel-keyed confirm path (they no
  longer need to carry op identity — the pending slot is authoritative). They are
  removed from help and from the preview messages. (Keeping them mapped to the
  same generic confirm intent is acceptable because the channel's pending op is
  authoritative; an alias does not need to "match" the pending op's kind.)
- An optional trailing repo substring on the legacy forms stays informational
  (the channel's pending entry is authoritative, exactly as today).

## Back-compat posture

`rollback-confirm` and `wipe-workspace-confirm` are kept as working DEPRECATED
ALIASES so an operator who already saw an old preview, or who has the old verb in
muscle memory, is not broken mid-flow. They are hidden from `help` and no longer
named in any preview text. Removal MAY follow in a later change once the new
preview text and help have been live long enough that the aliases see no use.

## Out of scope

`defer`/`undefer` (single-ack, no pending store) are unchanged and do NOT use
`confirm`.
