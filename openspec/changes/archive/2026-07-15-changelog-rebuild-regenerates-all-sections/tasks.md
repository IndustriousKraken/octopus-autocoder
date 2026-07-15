## 1. Verb argument

- [x] 1.1 In `autocoder/src/chatops/operator_commands.rs`, parse `--rebuild` into `ParsedChangelogArgs`; refuse the combination with `--since`/`--to` with a polite reply naming the conflict, before any state file is written.
- [x] 1.2 Unit tests for the parser: `--rebuild` alone parses; `--rebuild --since X` and `--rebuild --to Y` produce the refusal; existing arg forms are unchanged.

## 2. Rebuild payload

- [x] 2.1 In `autocoder/src/changelog_triage.rs`, extend `build_stylist_payload` with the rebuild branch: enumerate all stable tags, build one section per tag oldest-first (skipping the `documented_versions` subtraction), and mark the wrapped payload as a rebuild; keep the no-stable-tags NoOp.
- [x] 2.2 Unit tests: rebuild with three stable tags (two documented) yields three sections oldest-first with the rebuild marker; flagless behavior on the same fixture is unchanged (one section); no stable tags yields the NoOp reply for both modes.

## 3. Stylist guidance

- [x] 3.1 Update `prompts/changelog-stylist.md`: on a rebuild payload, replace each existing version section in place (chronological order), preserve the title/preamble/`## [Unreleased]`, and never leave two sections for the same version.
- [x] 3.2 Confirm the existing path-scope validation covers rebuild diffs without modification (still `CHANGELOG.md` plus `changelog:` frontmatter paths only).

## 4. Verification

- [x] 4.1 Run the full `cargo test` suite; confirm existing gap-fill scenarios (missing-only idempotence, pre-release skip, explicit-range bypass, no-op replies) are unchanged.
