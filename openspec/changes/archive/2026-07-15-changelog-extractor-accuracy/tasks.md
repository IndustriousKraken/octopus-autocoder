## 1. Tag-range resolution

- [x] 1.1 In `autocoder/src/cli/changelog.rs`, change default-`since` resolution to exclude tags pointing at the `--to` commit: list them via `git tag --points-at <to-commit>` and pass each as `--exclude` to `git describe --tags --abbrev=0 <to-commit>`; when nothing remains, keep the existing "ever" fallback and stderr notice.
- [x] 1.2 After resolution (explicit or defaulted), error out non-zero when `since_commit == to_commit`, naming both refs and the shared commit and suggesting an explicit `--since` / `--since ever`.
- [x] 1.3 Fixture tests: `--to <tag>` with no `--since` equals the explicit previous-tag run; equal explicit range exits non-zero with the expected message; single-tag repo with `--to <that-tag>` falls back to "ever" with the notice.

## 2. Issues-lane harvesting

- [x] 2.1 Extend the archive discovery walk to also match additions under `issues/archive/`, producing entries with `lane: "issue"` (changes get `lane: "change"`), slug from the directory or file-stem name.
- [x] 2.2 Read issue summaries from `issue.md` (first body paragraph after any leading heading), honoring the same `changelog:` frontmatter overrides (skip synonyms, summary override, unrecognized-value WARN).
- [x] 2.3 Render issue entries under a `### Fixes` markdown group after the capability groups; include `lane` on every JSON entry.
- [x] 2.4 Fixture tests: archived issue appears under Fixes with the right summary; `changelog: skip` on an issue lands it in `skipped`; a workspace without `issues/archive/` behaves exactly as today.

## 3. Unattributed-commit sweep

- [x] 3.1 Collect range commits via `git log --no-merges` and subtract the lane-adding commit SHAs already discovered; emit the remainder as `unattributed_commits` (`{sha, date, subject}`) in JSON and an `### Other changes` footer (`<short-sha> <subject>`) in markdown when non-empty.
- [x] 3.2 Fixture tests: a direct commit in range appears in `unattributed_commits` and the markdown footer; a range fully covered by lane commits emits no footer and an empty array; merge commits never appear.

## 4. Stylist integration

- [x] 4.1 Thread the new fields through the section JSON built in `autocoder/src/changelog_triage.rs` (the payload builder reuses the extractor's JSON rendering, so verify rather than duplicate).
- [x] 4.2 Update `prompts/changelog-stylist.md`: fold `Fixes` entries into the release section, classify or summarize `unattributed_commits` (bugfixes/docs/internal; omit trivia), and only describe a release as a maintenance/no-change release when entries, fixes, and unattributed commits are all empty.
- [x] 4.3 Run the full `cargo test` suite; confirm the existing changelog scenarios (frontmatter overrides, no-tags fallback, JSON shape, synthetic fixtures) still pass with the additive fields.
