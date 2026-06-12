# Wire issue-candidate promotion via `send it`

## Why

The canonical "Hybrid issue ingestion with maintainer promotion" requirement
states that a maintainer promotes a posted issue-lane candidate with a
`send it`, and that ONLY on promotion does the daemon write `issues/<slug>/`
and queue it. The ingestion half is implemented: a triaged report drafts a
candidate, persists its state, and posts it to chatops. The promotion half is
not wired. `promote_candidate` and `read_candidate` exist but are unreachable
(`#[allow(dead_code)]`), and the `send it` dispatcher recognizes only two
thread contexts — audit threads and brownfield-survey threads. An issue-
candidate thread matches neither, so a promotion attempt is refused with the
untracked-thread message.

Three concrete defects combine to make the documented flow unusable:

1. **No thread to match.** The candidate notification is posted fire-and-
   forget; it never captures the posted message's `thread_ts`. Audit threads
   and survey threads both persist their `thread_ts` so a later reply can be
   matched to the originating record. Candidates do not, so even a dispatcher
   that looked for them would have nothing to match against.

2. **No issue-candidate context in the `send it` dispatcher.** The dispatcher
   consults the audit-thread set and the survey set only. A `send it` in an
   issue-candidate thread falls through to the untracked-thread refusal.

3. **Misleading instruction.** The candidate notification tells the operator to
   "Reply `send it` in this thread", but the verb fires only on an `@<bot>`
   mention; a bare reply produces no event and the daemon does nothing. The
   audit and survey notifications both instruct `@<bot> send it`.

The brownfield-survey feature is the precedent: it added its second `send it`
context as an explicit `chatops-manager` requirement that routes the verb to a
distinct action. The issue-lane promotion was specified only as "reusing the
audit send-it pattern" in the orchestrator-cli requirement, with no explicit
dispatch requirement — which is how the wiring was left unbuilt.

## What Changes

- The candidate notification captures and persists the posted message's
  `thread_ts` and `channel` on the candidate's state record, so a later reply
  can be matched to it. The notification instructs `@<bot> send it`.

- The `send it` dispatcher gains a THIRD recognized thread context: an issue-
  candidate thread. On a fresh, posted candidate it promotes it (writes
  `issues/<slug>/`, queues it for the issues lane, marks the candidate
  promoted) and replies with the write-and-queue confirmation. On an already-
  promoted candidate it replies that no new work was taken. The untracked-
  thread refusal text names all three valid contexts.

- The daemon exposes a control-socket action that performs the promotion
  (write + queue + status flip) for a matched candidate, mirroring the
  existing audit-triage and brownfield-batch action handlers.

No change to the ingestion, triage, classification, dedup, or quarantine
behavior; no change to the issues-lane walker or precedence. This change wires
the existing promotion primitive to the existing verb.

## Impact

- Affected specs: `chatops-manager` (new third `send it` context),
  `orchestrator-cli` (the ingestion requirement's promotion wiring made
  explicit: thread capture, the promotion control-socket action, the
  corrected instruction text).
- Affected code: `lanes/ingestion.rs` (capture `thread_ts`/`channel` at post
  time; the `read_candidate`/`promote_candidate` primitives become reachable),
  `chatops/operator_commands.rs` (issue-candidate `send it` context),
  `control_socket.rs` (the promotion action handler), and the candidate
  notification text.
- Governing invariant: the canonical "Hybrid issue ingestion with maintainer
  promotion" requirement — this change brings the implementation into
  conformance with it.
