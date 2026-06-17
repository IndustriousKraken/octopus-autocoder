# Fix: reported-issue ingestion re-spams chatops for behavior-change reports

## Report

A reported GitHub issue that triage classifies as a behavior change causes the
ingestion driver to post the same chatops message every polling pass, without end
(observed: coterie reported issue #67, the `↪️ … wants a behavior change — routing
to the changes lane (openspec/changes/) as a proposal, NOT an issue.` message
repeating every 15-30 minutes). The message also claims a routing action that no
code performs.

## Diagnosis

In `autocoder/src/lanes/ingestion.rs`:

- `run_issue_ingestion` fetches OPEN reported issues each pass and skips a report
  only when `candidate_exists(state_root, id)` is true (~L834). That state is
  written ONLY by the `IssueCandidate` route, inside `post_candidate`.
- `act_on_verdict`'s `TriageRoute::ChangesProposal` arm (~L791-802) and the
  `Declined` arm persist NOTHING to disk; the caller collects an in-memory
  `outcomes` vector (~L889) and discards it.
- Result: a report classified `BehaviorChange` never records a disposition, so
  every pass re-fetches it (still open on GitHub), re-triages it, and re-posts the
  message. The `candidate_exists` gate's own doc comment states its purpose is "so
  the ingestion pass does not re-triage / re-post it" — but it only guards the one
  route that writes state.
- Separately, the `ChangesProposal` arm only posts a message and returns; it
  drafts no proposal under `openspec/changes/`, so "routing to the changes lane as
  a proposal" describes an action that does not happen. The duplicate check
  (`is_duplicate`) also runs only in the `IssueCandidate` arm, so the behavior-
  change route has no dedup.

## Acceptance criteria

Stated against the existing `Hybrid issue ingestion with maintainer promotion`
requirement (orchestrator-cli), which already specifies that ingestion SHALL
"triage reported GitHub issues read-only, classify AND dedup each against open AND
archived issues, draft a candidate, AND post … WITHOUT queuing." This fix makes the
code conform; it adds no new contract and changes no observable interface.

1. **Each reported issue is triaged at most once.** After a report reaches any
   terminal disposition (candidate posted, routed-to-changes, declined, or
   deduped), a subsequent ingestion pass over the same still-open report does NOT
   re-triage it AND does NOT re-post any chatops message for it. (Today only the
   candidate route is idempotent; this extends the same already-intended idempotency
   — see the `candidate_exists` doc comment — to every route.)

2. **Dedup runs for the behavior-change route.** The duplicate check (currently
   only in the candidate route) also runs before the behavior-change route posts:
   a report that duplicates an existing issue is deduped — no routing message is
   posted — per the existing "dedup each against open AND archived issues" contract,
   which today the behavior-change route skips entirely.

3. **The behavior-change route states only what it does.** Until a real
   changes-candidate flow exists, the behavior-change route posts at most once AND
   its message does not claim that autocoder routed or created a proposal; it states
   honestly that the report needs a maintainer-authored proposal. (Message wording
   is not asserted by any test, per the project's test-behavior-not-wording rule.)

## Out of scope

Actually drafting a behavior-change report into a promotable changes-lane candidate
(mirroring the issue-candidate promotion flow, landing in `openspec/changes/<slug>/`
on `send it`) is a NEW capability and belongs in a separate spec change — it changes
what public reports can produce. This issue only stops the re-spam, adds the missing
dedup, and makes the message honest, all of which are behavior-preserving fixes to
match the existing contract.
