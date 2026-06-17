# Design

## D1 — Scope: contradiction markers only

The `.needs-spec-revision.json` marker has two sources: the executor's
unimplementable-tasks flag (populated `unimplementable_tasks`) and the `[in]` /
`[canon]` contradiction gates (empty `unimplementable_tasks`, a contradiction-
derived `revision_suggestion`, and — on a gate-error hold — a populated
`gate_error`). a03 acts ONLY on the contradiction markers (empty
`unimplementable_tasks`, no `gate_error`). The unimplementable-tasks flow keeps its
existing invariant untouched: the agent flags, the operator authors the tasks.md
edit. a03 detects the marker kind from its content and offers the revision thread
only for contradiction markers.

## D2 — The thread is the conversation store; sessions are stateless

a03 stores no agent session. The contradiction alert's `channel` and `thread_ts`
are captured in a `RevisionThreadState` (keyed to repo + change slug, mirroring
`AuditThreadState`), which is the only persisted state — it exists so a later reply
can be matched to its change. Each operator reply reconstructs a fresh agent from
artifacts that already exist on disk and in chat: the change's spec deltas, the
relevant canon, the marker's contradiction, and the Slack thread transcript so
far. A daemon restart mid-discussion loses nothing; the next reply rebuilds the
context. This is the same stateless, artifact-grounded model the audit self-heal
loop uses — no session id to store, no session lifecycle to clean up.

## D3 — Two roles in the thread: advisor (read-only) and executor (write+PR)

- **Advisor** — a non-`send it` `@<bot>` reply in a revision thread runs a
  read-only agentic session (`Read`/`Glob`/`Grep`, no write) that answers the
  operator's question. It reads the change, the canon, and the contradiction and
  discusses the choice — typically align-the-change-to-canon vs MODIFY-the-canonical-
  requirement, and how — but writes nothing. Multiple rounds are just multiple
  replies, each rebuilding from the (now longer) transcript.
- **Executor** — `@<bot> send it` runs a write-scoped session that edits the
  change's spec deltas along the direction the thread converged on, then re-runs the
  `[in]` and `[canon]` gates (a02's invocation) against the revised change. On a
  clean re-gate it opens a PR with the spec revision and reports the PR link in the
  thread. On a still-failing re-gate it opens NO PR and reports the remaining
  contradiction in the thread (the operator can discuss further and `send it`
  again).

## D4 — Human direction and human review (the marching-orders invariant)

The project invariant is that no AI process modifies its own marching orders
without human review. a03 honors it by construction: the operator directs the
approach in the discussion, triggers the rewrite explicitly with `send it`, and
reviews the resulting PR before it merges. The agent drafts; it never
autonomously commits a spec revision to the base branch. The executor revises the
change's spec deltas (proposal/specs) to resolve a contradiction; it does NOT
auto-edit a `tasks.md` to dodge the executor's unimplementable-tasks flag — that
separate marker keeps its operator-authors invariant. a03 thus adds a reviewed
path for contradiction markers without relaxing the autonomy boundary.

## D5 — Re-gate before PR closes the loop

The executor re-runs the same `[in]`/`[canon]` checks a02 uses, against the revised
change, BEFORE opening the PR. An operator-directed revision cannot itself ship a
new contradiction: either the re-gate is clean and a PR opens, or it is not and the
thread is told. This makes the revision path self-verifying, the same way a02's
authoring-time self-heal is.

## D6 — `send it` as the fourth context

The dispatch (per `wire-issue-candidate-promotion`) looks a reply's `thread_ts` up
against the audit, brownfield-survey, and issue-candidate sets. a03 adds a fourth:
the revision-thread set (`RevisionThreadState.thread_ts`). A `send it` matching a
revision thread runs the revision executor; a reply that is not `send it` runs the
advisor; a reply matching none of the four sets gets the untracked-thread refusal,
now naming all four contexts. The `clear-revision` verb is unchanged — the operator
can still resolve a marker by hand and clear it directly.

## D7 — Relationship to a02

a02 self-heals audit-authored changes before they are committed, so most audit
findings never reach a marker. a03 serves what remains: hand-written changes that
trip the implement-time gates, and audit changes where the authoring-time gate was
disabled or missed something. Wiring a02's fail-closed residue directly into an
a03 thread (so an exhausted audit finding becomes a discussable thread rather than
only a surfaced alert) is a natural follow-on but is out of scope here; a03 acts on
the marker that the implement-time gates already produce.
