# Tasks

## 1. Record a disposition for every reported issue (idempotency)

- [ ] 1.1 Persist a terminal disposition for EACH reported issue after triage, keyed by `candidate_id(repo_url, number)` — not just the `IssueCandidate` route. Reuse the `candidates_dir` keying. Either extend the persisted candidate record with a disposition that covers the non-candidate routes (e.g. `RoutedToChanges`, `Declined`, `Duplicate`) — keeping the candidate-specific fields optional for those — OR write a lightweight sibling disposition marker (`<state>/issue-candidates/<id>.json` already exists for posted candidates; a parallel `<id>.disposition.json`, or a `status` variant, is acceptable). Keep writes atomic (tempfile-then-rename, like `write_candidate`).
- [ ] 1.2 In `act_on_verdict`, write the disposition for the `ChangesProposal`, `Declined`, and duplicate paths (the `PostedCandidate` path already persists via `post_candidate`; the dedup path currently returns `Declined` without persisting — persist it too).
- [ ] 1.3 In `run_issue_ingestion`, change the pre-triage skip (currently `candidate_exists`) to an "already-dispositioned" check that returns true when ANY terminal disposition is recorded for the report's id, so a report is triaged once and not re-triaged on later passes. Preserve the existing `AlreadyHandled` outcome for the skip.

## 2. Dedup the behavior-change route

- [ ] 2.1 In the `ChangesProposal` arm of `act_on_verdict`, run the existing duplicate check (`is_duplicate` against `existing_issue_slugs`, open AND archived) BEFORE posting any message — the same check the `IssueCandidate` arm already performs. On a duplicate, post the existing `🔁 … deduped` style notification (or none) and record a `Duplicate`/`Declined` disposition instead of the routing message.
- [ ] 2.2 (Deferred to the future changes-candidate flow, NOT this issue) dedup against existing `openspec/changes/` slugs — meaningful only once behavior-change reports can produce changes-lane candidates.

## 3. Make the behavior-change message honest and one-time

- [ ] 3.1 Change the `ChangesProposal` arm's message so it does not claim autocoder routed or created a proposal. State that the report appears to need a maintainer-authored change proposal (autocoder does not auto-draft changes from public reports). The disposition marker from §1 ensures it posts at most once.
- [ ] 3.2 Do not assert the message wording in any test (per `Tests assert behavior or derivation, never message wording`).

## 4. Tests

- [ ] 4.1 A behavior-change report is triaged AND notified exactly once across multiple ingestion passes over the same still-open report (assert: the disposition is recorded after pass 1; pass 2 yields `AlreadyHandled` and triggers no executor run and no notification). Derive from state + call counts, not message text.
- [ ] 4.2 A behavior-change report whose slug duplicates an existing issue is deduped: no routing message, a `Duplicate`/`Declined` disposition is recorded.
- [ ] 4.3 The disposition record round-trips (write then read) for each non-candidate route; an unparseable or absent record is handled gracefully (absent → not yet dispositioned).
- [ ] 4.4 Regression: the `IssueCandidate` route still writes its candidate state and is still skipped on the next pass (existing idempotency unchanged); the promotion flow is unaffected.

## 5. Notes

- Re-triaging a report after its GitHub body is EDITED (e.g. keyed on a content hash) is intentionally out of scope; the disposition is recorded once and stands until cleared.
