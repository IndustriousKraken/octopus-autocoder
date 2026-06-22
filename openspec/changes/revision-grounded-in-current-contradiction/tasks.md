# Tasks

OpenSpec: implements the MODIFIED "Send it in a revision thread runs the
spec-revision executor" requirement AND the ADDED "The spec-revision marker
carries the current contradiction set" requirement in
`specs/orchestrator-cli/spec.md`.

## 1. Marker carries structured findings

- [ ] 1.1 In `autocoder/src/spec_revision.rs`, extend the marker so it can carry
  the contradiction findings in a structured form — each with the conflicting
  requirement identity (for a `[canon]` finding, the conflicting canonical
  capability AND requirement title; for an `[in]` finding, the two conflicting
  requirements). Reuse the existing gate finding structs
  (`CanonContradictionFinding` / the `[in]` finding) rather than inventing a new
  shape. Keep the existing `revision_suggestion` prose field (back-compat); the
  structured findings are additive. A marker written by an older daemon (no
  structured findings) SHALL still parse.

## 2. Re-gate returns structured findings

- [ ] 2.1 In `autocoder/src/polling/revision_session.rs`, change `ReGateOutcome`
  so its `Contradiction` variant carries the structured findings from the `[in]`
  and `[canon]` checks (not only the formatted string). `GatesReGateRunner::regate`
  already has the `ContradictionCheckOutcome::Found(findings)` /
  `CanonContradictionCheckOutcome::Found(findings)` vectors in hand — thread them
  out instead of discarding them after formatting the message.

## 3. Refresh the marker on a re-gate that still contradicts (durable source of truth)

- [ ] 3.1 In `run_revision_execute`, on `ReGateOutcome::Contradiction(...)`,
  refresh `.needs-spec-revision.json` with the re-gate's current structured
  findings (replacing the prior set) before reporting back. Best-effort: log and
  continue on write failure; never change the revision outcome. The marker is
  gitignored and survives `restore_base` (it is untracked + excluded), so the
  refresh persists for the next `send it`.
- [ ] 3.2 The refresh updates findings only — it does NOT clear the marker and is
  NOT staged into any PR (the existing `git reset` of the marker path stays).

## 4. Executor is grounded in the current marker findings (resolve all)

- [ ] 4.1 In `build_executor_prompt`, read the marker's structured findings and
  enumerate them in the prompt as the set the revision MUST resolve ("resolve each
  of these contradictions: …"), so the executor addresses every recorded
  contradiction, not only the one in the prose narrative. The thread transcript
  remains the source of the operator's chosen direction; the marker is the source
  of what must be resolved.

## 5. Never revise blind: bounded transcript fetch + fail-closed abort

- [ ] 5.1 In `process_pending_revision_execute`, wrap `fetch_thread_transcript`
  in a bounded retry (`executor.revision_transcript_fetch_retries`, small default
  e.g. `2`, with short backoff). On success, proceed as today.
- [ ] 5.2 On persistent failure, do NOT degrade to an empty transcript and run the
  revision. Post a legible thread reply ("could not read the discussion thread —
  not revising blind; `send it` again in a moment") AND return without invoking
  the edit session or opening a PR. Remove the current silent
  `Vec::new()` degrade on the executor path.
- [ ] 5.3 The read-only advisor (`process_pending_revision_advise`) keeps
  answering when the transcript cannot be read (it writes nothing), but SHALL
  surface that it answered from a degraded/partial thread. Apply the same bounded
  retry there; do not abort the advisor.

## 6. Bounded converge loop within one send it + escalation

- [ ] 6.1 Add config `executor.revision_converge_attempts` (additional in-`send
  it` edit→re-gate attempts beyond the first; small default e.g. `2`; `0` keeps
  the current single-pass behavior). Model on the existing small-integer executor
  config fields.
- [ ] 6.2 In `run_revision_execute`, wrap the edit → scope-check → re-gate in a
  bounded loop. On `ReGateOutcome::Contradiction` with budget remaining, re-run
  the edit session (its prompt now includes the latest re-gate findings via the
  refreshed marker, task 3/4) and re-gate again on the same revision branch,
  accumulating fixes. Only `restore_base` + report back when the budget is
  exhausted (or on a scope violation / could-not-run, which are terminal as
  today). On a clean re-gate at any iteration, open the PR.
- [ ] 6.3 Escalation: track the re-gate finding identity across iterations (and,
  via the refreshed marker from task 3, across `send it`s). When the SAME finding
  identity survives the bounded attempts, the report names that specific
  requirement and states the revision is not clearing it, instead of an identical
  generic "still fails" message. Derive the escalation from the structured finding
  identity, not from string-matching the message text.

## 7. Tests

- [ ] 7.1 Marker refresh (task 3): an injected re-gate runner returning a
  `Contradiction` with findings distinct from the marker's prior set results in
  the marker recording the NEW findings (assert the marker's structured findings,
  not message wording). A marker with no structured findings still parses (1.1).
- [ ] 7.2 Fail-closed transcript (task 5): a transcript runner that fails every
  attempt → the edit session is NOT invoked, no PR is opened, and a thread reply
  is posted; a runner that fails then succeeds within the retry budget → the
  revision proceeds. Assert the edit-runner invocation count / PR outcome, not
  wording.
- [ ] 7.3 Resolve-all grounding (task 4): given a marker with two structured
  findings, the executor edit session is handed both findings to resolve (assert
  the data flow — both finding identities are threaded to the session — via the
  injected edit runner that records its input; do not assert prose wording).
- [ ] 7.4 Converge loop (task 6): an injected re-gate runner returning
  `Contradiction` then `Clean` opens a PR within one `send it` (edit runner called
  twice, PR opened, no second operator trigger). With
  `revision_converge_attempts: 0`, the first contradiction reports back as today.
- [ ] 7.5 Escalation (task 6.3): a re-gate runner returning the SAME finding
  identity across the bounded attempts produces an exhaustion report carrying that
  finding's identity (assert the outcome/finding identity, not message wording).
- [ ] 7.6 Config: `revision_converge_attempts` and
  `revision_transcript_fetch_retries` default when absent; explicit values parse;
  `revision_converge_attempts: 0` preserves single-pass behavior.

## 8. Validation

- [ ] 8.1 `cd autocoder && cargo test --bin autocoder` (the suite is known-flaky
  under parallel load — re-run / isolate any failure before treating it as real).
- [ ] 8.2 `openspec validate revision-grounded-in-current-contradiction --strict`
  from the repo root.
