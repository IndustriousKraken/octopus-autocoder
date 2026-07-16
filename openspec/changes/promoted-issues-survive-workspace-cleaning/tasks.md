## 1. Shared unit-writer

- [ ] 1.1 In `autocoder/src/lanes/ingestion.rs`, extract the unit-writing core of `promote_candidate` (form decision, file contents, atomic writes) into a helper both promotion and reconciliation call, so re-materialized units are byte-identical to the original promotion.

## 2. Reconciler

- [ ] 2.1 Add a reconcile function: scan the candidate store for records with status promoted matching the repository URL; for each, check unit presence in `issues/` (both forms) and `issues/archive/` (date-prefixed entries ending in the slug, both forms); re-materialize absent units via the shared writer, logging one WARN per resurrection naming the slug.
- [ ] 2.2 Call the reconciler once per polling iteration before the issues-lane `list_ready`, gated the same way the lane itself is (skip when `features.issues.enabled` is false).

## 3. Tests

- [ ] 3.1 Unit test: a promoted record with no workspace unit and no archive entry is re-materialized with identical content and form (curated single-file AND public-origin directory variants, including `report-body.md`).
- [ ] 3.2 Unit test: a record whose unit exists in `issues/` is untouched; a record whose slug matches an `issues/archive/<date>-<slug>` (or `.md`) entry is not re-materialized.
- [ ] 3.3 Unit test: a deleted record file means no re-materialization (tombstone semantics).
- [ ] 3.4 Regression test for the observed failure shape: promote, simulate a `git clean`-style deletion of the untracked unit, run the reconciler, and assert the unit is back and `list_ready` includes it.
- [ ] 3.5 Run the full `cargo test` suite; confirm existing promotion-idempotence and ingestion tests pass unchanged.
