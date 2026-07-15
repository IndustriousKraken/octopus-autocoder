## Why

Existing changelogs generated before the extractor learned to see issues-lane fixes and out-of-lane commits (and before the degenerate-range fix) understate or misstate what shipped — e.g. sections claiming "no changes" for windows that held security fixes. Flagless gap-fill is deliberately idempotent (it only adds missing versions), so there is no way to repair the already-documented sections short of hand-editing or deleting the file. Operators need a one-verb way to regenerate the whole changelog from history under the current, more complete coverage rules.

## What Changes

- The `changelog` chatops verb accepts a `--rebuild` argument: every stable release tag is documented from scratch, oldest-first — sections for already-documented versions are regenerated and replaced in place, not skipped.
- `--rebuild` is mutually exclusive with `--since`/`--to` (polite refusal naming the conflict).
- Everything else about the flow is unchanged: same extractor, same stylist, same path-scope validation, same single PR, same revision loop — the operator reviews the full regenerated changelog as one diff before it lands.
- Non-version content (title, preamble, `## [Unreleased]`) is preserved; only version sections are regenerated.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `orchestrator-cli`: the "The `changelog` chatops verb defaults to tag-driven gap-fill" requirement gains the `--rebuild` mode (regenerate all stable-tag sections) alongside the existing flagless (missing-only) and explicit-range behaviors.

## Impact

- `autocoder/src/chatops/operator_commands.rs`: parse `--rebuild` in the changelog verb args.
- `autocoder/src/changelog_triage.rs`: `build_stylist_payload` gains the rebuild branch (all stable tags → one section each, ignoring `documented_versions`); the stylist payload/prompt marks the run as a rebuild so the stylist replaces existing sections instead of inserting duplicates.
- `prompts/changelog-stylist.md`: rebuild guidance (replace version sections in place, preserve preamble and Unreleased).
- Depends on `changelog-extractor-accuracy` for the coverage the rebuild exists to apply; mechanically it only needs the extractor's JSON, so the changes can land in either order.
