# Tasks

## 1. Relocate the constant + lifecycle helpers

- [ ] 1.1 In `lanes/issues.rs`, change `ISSUES_SUBDIR` from `"openspec/issues"` to `"issues"`. `issues_dir`, `issue_dir`, and `archive_root` derive from it, so the active path, per-slug path, and archive path all move together. Update the module-level doc comments (they cite `openspec/issues/<slug>/` and `openspec/issues/archive/`) to the root paths.

## 2. Migration for repositories already holding `openspec/issues/`

- [ ] 2.1 Add a transitional fallback so a repo whose tree has not yet been moved keeps working for one release: when the new `issues/` directory is absent AND a legacy `openspec/issues/` directory exists, `issues_dir`/`archive_root` resolve to the legacy location (read AND write). When `issues/` exists, the legacy path is ignored. This avoids a flag-day across managed repos. Log a one-time WARN naming the legacy path AND the remedy (`git mv openspec/issues issues`).
- [ ] 2.2 Document the migration for operators: a one-time `git mv openspec/issues issues` per repository (moves active AND archived units together, preserving history); the fallback covers repos that lag the deploy; the fallback is removed in a later release.
- [ ] 2.3 Confirm the walker still excludes `archive/` (it does — `list_ready` skips `ARCHIVE_DIR`), so archived units are never re-worked by the relocation; and confirm ingestion dedup (`existing_issue_slugs`, which reads `archive_root`) follows the relocated/fallback path so archived slugs remain deduped.

## 3. Update agent-facing prompts

- [ ] 3.1 Update the hardcoded `openspec/issues/<slug>/` strings in the audit/issue prompts to `issues/<slug>/`: `audits/specs_writing.rs` (the inlined `openspec/issues/<slug>/` guidance), `prompts/audit-triage.md`, `prompts/missing-tests-audit.md`, `prompts/security-bug-audit.md`. The agent must be told to write to the canonical root path.

## 4. Update operator docs

- [ ] 4.1 Update `docs/OPERATIONS.md`, `docs/CONFIG.md`, and `docs/CHATOPS.md` references from `openspec/issues` to `issues/`.

## 5. Update tests

- [ ] 5.1 Update tests that hardcode `openspec/issues/...` to the root path (or, preferably, route them through `lanes::issues::{issues_dir, issue_dir, archive_root}` so they follow the constant): `audits/security_bug.rs`, `audits/specs_writing.rs`, `audits/scheduler.rs`, `control_socket.rs`. Add a test asserting `issues_dir` resolves under `issues/` (not `openspec/`), and a fallback test (legacy `openspec/issues/` present, new `issues/` absent → resolves to legacy).

## 6. Verify

- [ ] 6.1 Grep the tree: no `openspec/issues` literal remains in `autocoder/src` (outside the migration-fallback code path), `prompts/`, or `docs/`.
- [ ] 6.2 End-to-end: an issue committed at `issues/<slug>/` is worked and archived to `issues/archive/<date>-<slug>/`; a repo with a legacy `openspec/issues/` tree (active + archived) still works its active issue AND dedups against its archived one.
