# Wire issue-candidate promotion via `send it`

## Why

The canonical "Hybrid issue ingestion with maintainer promotion" requirement
states that a maintainer promotes a posted issue-lane candidate with a
`send it`, and that ONLY on promotion does the daemon write `issues/<slug>/`
and queue it. The ingestion half is implemented: a triaged report drafts a
candidate, persists its state, and posts it to chatops. The promotion half is
not wired. `promote_candidate` and `read_candidate` exist but are unreachable
(`#[allow(dead_code)]`).

Canon's `send it` requirements already describe an issue-candidate thread
context: the `Inbound listener routes send it to BrownfieldBatchAction when
posted in a brownfield-survey thread` requirement enumerates a four-context
lookup (audit, brownfield-survey, issue-candidate, AND spec-revision) and
references an "issue-candidate promotion" requirement for the branch — but that
requirement was never defined, only cited, and the dispatcher was never wired
for it. The `send it` dispatcher today resolves the audit-thread, survey, and
spec-revision sets only; an issue-candidate thread matches none, so a promotion
attempt is refused with the untracked-thread message even though that message
advertises issue-candidate as a valid context. Canon and the operator-facing
text describe behavior the code does not implement.

Three concrete defects combine to make the documented flow unusable:

1. **No thread to match.** The candidate notification is posted fire-and-
   forget; it never captures the posted message's `thread_ts`. Audit threads
   and survey threads both persist their `thread_ts` so a later reply can be
   matched to the originating record. Candidates do not, so even a dispatcher
   that looked for them would have nothing to match against.

2. **No issue-candidate branch in the `send it` dispatcher.** The dispatcher
   consults the audit-thread, survey, and spec-revision sets. A `send it` in an
   issue-candidate thread falls through to the untracked-thread refusal — which
   names the issue-candidate context as valid, so the refusal contradicts
   itself.

3. **Misleading instruction.** The candidate notification tells the operator to
   "Reply `send it` in this thread", but the verb fires only on an `@<bot>`
   mention; a bare reply produces no event and the daemon does nothing. The
   audit and survey notifications both instruct `@<bot> send it`.

The brownfield-survey and spec-revision features are the precedent: each defined
its `send it` context as an explicit `chatops-manager` requirement that routes
the verb to a distinct action. The issue-lane promotion was specified only as
"reusing the audit send-it pattern" in the orchestrator-cli requirement, with no
explicit dispatch requirement — which is how the wiring was left unbuilt while
canon's enumeration assumed it existed.

## What Changes

- A new `chatops-manager` requirement DEFINES the issue-candidate `send it`
  branch that canon already references: on a fresh, posted candidate it promotes
  it (writes `issues/<slug>/`, queues it for the issues lane, marks the
  candidate promoted) and replies with the write-and-queue confirmation; on an
  already-promoted candidate it replies that no new work was taken; the branch
  runs after the audit and survey lookups and before the spec-revision lookup,
  matching canon's stated context order.

- The candidate notification captures and persists the posted message's
  `thread_ts` and `channel` on the candidate's state record, so a later reply
  can be matched to it. The notification instructs `@<bot> send it`.

- The daemon exposes a control-socket action that performs the promotion
  (write + queue + status flip) for a matched candidate, mirroring the
  existing audit-triage and brownfield-batch action handlers.

This change does NOT redefine the four-context lookup, the untracked-thread
refusal, or the help text — canon already carries those (they enumerate
issue-candidate). It defines the missing branch and wires the code to match.
No change to the ingestion, triage, classification, dedup, or quarantine
behavior; no change to the issues-lane walker or precedence.

## Impact

- Affected specs: `chatops-manager` (the issue-candidate `send it` branch,
  defined for the first time), `orchestrator-cli` (the ingestion requirement's
  promotion wiring made explicit: thread capture, the promotion control-socket
  action, the corrected instruction text).
- Affected code: `lanes/ingestion.rs` (capture `thread_ts`/`channel` at post
  time; the `read_candidate`/`promote_candidate` primitives become reachable),
  `chatops/operator_commands.rs` (issue-candidate `send it` branch inserted
  between the survey and spec-revision lookups), `control_socket.rs` (the
  promotion action handler), and the candidate notification text.
- Governing invariant: the canonical "Hybrid issue ingestion with maintainer
  promotion" requirement — this change brings the implementation into
  conformance with it, and closes the canon-vs-code drift left when canon's
  four-context `send it` enumeration shipped ahead of the issue-candidate
  branch.
