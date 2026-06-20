# Contributing

## Source files and functions stay within a size budget

Source-file and function length are a **maintainability budget**, not just a
metric an audit happens to report. Keep them in mind as you write:

- A source file **should** stay at or under roughly **500 lines**.
- A function **should** stay at or under roughly **50 lines**.

These are **judgment targets, not hard caps**. Genuinely cohesive,
single-responsibility code *may* exceed them when splitting would only add
indirection without reducing complexity — the test is **cohesion, not the
line count**. A large file that does exactly one thing is fine; a smaller
file that mixes three unrelated concerns is not.

### When it becomes a concern

When a file or function grows well past the budget AND mixes unrelated
responsibilities, treat it as a refactor worth doing — and the concern grows
the further over it goes and the more concerns it mixes. The test is always
cohesion, not the raw line count: a long file that does one thing is fine; a
shorter file that mixes three unrelated concerns is the one to split.

**Duplicated logic is likewise a concern.** Near-identical function bodies, or
one intent reimplemented across files, are worth collapsing — in an LLM-grown
codebase the bloat is *reachable* (it passes a dead-code linter), so prefer one
parameterized helper over a family of copy-paste clones. Because duplication
spans the whole tree rather than one file, surfacing it is a corpus-level
concern, not the per-file advisor's.

### Surfacing is advisory, never a gate

Size and structure are surfaced in two places, neither of which blocks:

- **`architecture_advisor` audit** — samples the longest files over a
  configurable pain threshold and, by cohesion judgment, recommends refactoring
  the worst offenders (a ranked, anchored list — the line count is a selector,
  never a finding). Acting on a recommendation via `@<bot> send it` drafts a
  behavior-preserving **issue** by default.
- **Code review** — adds an advisory note when a pass enlarges an over-budget
  file or function (it does not penalize a pass that shrinks one).

A size finding **never, on its own, blocks a pull request or a change from
archiving**. It is a maintainability signal that informs prioritization, not a
correctness gate. The size budget has a single advisory home (this section);
audits and triage do NOT mint a new requirement that restates the threshold —
a behavior-preserving refactor is an issue, not a spec encoding a metric.

## Control-plane gatekeepers fail closed

A **control-plane gatekeeper** is any component whose job is to decide whether
work may proceed, or to attest that work meets a standard: the pre-flight
contradiction gates (`[in]`, `[canon]`), the code-implements-spec gate
(`[out]`), the code reviewer, any future verifier, and the audits that gate an
operator's `send it`. The invariant for every one of them:

> **An inability to run is a distinct, surfaced, non-passing state — never a
> pass.** A control that fails *open* (treats "I could not run" as "everything
> is fine") is not a control; it silently removes the rail while reporting
> green.

This is a canonical requirement (`project-documentation` → *Control-plane
gatekeepers fail closed, never to a passing verdict*), so the periodic
`drift_audit` and the `[canon]` gate read it and can flag a new gatekeeper that
defaults to pass. Apply it whenever you add or change a gate — these are the
exact traps it has caught before (each shape is "an inability-to-run collapsing
into a passing verdict"):

- **Verdict defaults and initializers are the non-passing state.** A verdict
  variable, accumulator, or struct default initializes to blocked / errored /
  unknown — never to approve / pass. `ContradictionCheckOutcome` has variants
  `Clean` / `Found` / `Errored` with no default-approve; the verdict ledger's
  `GateVerdict` is default-deny — "open" requires an affirmative, completed
  `Pass` (a crash or unhandled path leaves the gate non-passing).
- **Zero-item aggregations are non-passing.** An aggregation over zero
  evaluated items does not yield a pass (the empty-session bug). "I reviewed
  nothing, so everything's approved" is the exact failure this forbids.
- **Error paths do not collapse into pass.** A spawn or timeout failure, an
  unavailable / unregistered CLI, a missing or unparseable result, a
  schema-rejected submission the agent never corrects, or "no result recorded"
  is treated as **errored** — never as "no findings" / "approved" / "verified".
- **The errored state is operator-visible.** Surface it via chatops and/or the
  artifact the gatekeeper writes, naming the gatekeeper and the cause, so "ran
  and passed" is distinguishable from "could not run".

### The action on error follows the gatekeeper's role

| Role | On a can't-run error | Example |
| ---- | -------------------- | ------- |
| **Blocking** | Hold the gated work in an explicit failed-to-run state an operator clears — distinct from a "found a problem" verdict. Do NOT let work proceed as if it passed. | `[in]` / `[canon]` record `GateVerdict::FailedToRun`; the executor runs only when every blocking gate is `Pass` or `Disabled`, so the change is held. |
| **Advisory** | Render an explicit "failed to run" result rather than omitting output or reporting success. Never blocks. | `[out]` renders a `## Spec Verification: FAILED TO RUN — <cause>` PR-body section instead of dropping the section. |

The code reviewer is the reference conformant case: a session with no valid
submission returns `Discarded { reason }` — it never defaults to `Approve`.

### Transient tolerance is bounded retry, then errored

Where a gate retries a transient blip (e.g. a flaky no-submission session) to
avoid wedging on a one-off, it retries a **bounded** number of times
(`executor.verifier_gate_retries`, default `2`) and then enters the errored
state. Retrying forever, or falling through to pass once the bound is
exhausted, both violate the invariant.

### Second axis: the gatekeeper contains no judgment

Everything above closes one fail-open axis — *an inability to run collapsing
into a pass*. There is a second, independent axis: the code that **manufactures
the verdict** itself. A gatekeeper can satisfy every clause above — no error, no
default, no zero-item aggregation — and still pass work that nothing evaluated,
because the code decided the outcome instead of an agent. A pass nothing judged
is fail-open by construction even though nothing errored.

For a gatekeeper whose verdict is a matter of judgment (the reviewer, a
verifier, an audit that gates `send it`), that judgment belongs to an agent, not
to the code. The code's responsibilities are **exactly four**:

1. **Initialize to the non-passing state** (the first axis).
2. **Assemble the inputs** — the prompt and the materials to evaluate.
3. **Invoke the agent.**
4. **Surface the agent's returned verdict verbatim.**

The code synthesizes **no** verdict. Two traps follow:

- **The code never derives the verdict by inspecting the inputs.** A
  code-authored conclusion — most often *"the materials to evaluate are absent
  or empty, so this passes"* — is a **manufactured pass**, not a judgment, even
  though no error occurred. When the inputs make a genuine agent evaluation
  impossible, the outcome is the **failed-to-run** state (first axis), never a
  synthesized pass. Example non-conformance to avoid: an `[out]` gate that finds
  no spec delta and emits a pass instead of failing to run (and locating the
  delta at its archived path).
- **The agent is never handed an option set that forecloses failure.** The
  verdict mechanism — the MCP submission tool's schema *and* the prompt that
  frames it — must let the agent express a **failing** verdict with its own
  **prose alone**. Structured detail (gap lists, concern arrays) may accompany a
  verdict but is **never a precondition** for returning a failing one. A prompt
  or schema built so that pass is the only expressible answer is fail-open by
  construction. Example non-conformance to avoid: a verdict mechanism that
  requires structured detail before a failing verdict can be submitted.

The reviewer is again the reference shape: it assembles the diff and prompt,
invokes the agent, and surfaces what the agent returns — a session with no valid
submission is `Discarded { reason }`, never a code-chosen `Approve`. Apply both
axes whenever you add or change a gate; the canonical requirement
(`project-documentation` → *Control-plane gatekeepers fail closed, never to a
passing verdict*) carries both, so `drift_audit` and the `[canon]` gate read it.
