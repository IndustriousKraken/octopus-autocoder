## Why

The changelog extractor produces confidently wrong output in two ways. First, when `--to` names a tag and `--since` is unset, the default `since` resolves via `git describe --tags --abbrev=0 <to>` — which returns the `to` tag itself, yielding the empty range `(vX .. vX]` and a "no changes" changelog for a release that shipped real work (observed: autocoder's own v1.3.1 PR claimed "no changelog-tracked changes" while the window held five archived changes and four security fixes). Second, the harvester reads only `openspec/changes/archive/` — issues-lane corrections (`issues/archive/`) and commits outside both lanes are invisible, so releases whose substance was bugfixes (e.g. a notable timezone fix, or a batch of security fixes) report as empty.

## What Changes

- The default `--since`, when `--to` resolves to a commit that a tag points at, excludes that tag itself and resolves to the most recent tag strictly before it — so `changelog --to v1.3.1` documents `(v1.3.0 .. v1.3.1]`, not an empty self-range.
- A resolved range whose `since` and `to` are the same commit is a hard error (exit non-zero, naming both refs and suggesting an explicit `--since`), never an empty-success "no changes" document.
- The harvester also walks `issues/archive/` in the same tag range: each archived issue becomes an entry (summary from its `issue.md`, same `changelog:` frontmatter overrides), grouped under a `Fixes` heading in markdown and tagged `lane: "issue"` in JSON.
- The extractor reports commits in the range that belong to neither lane (added no archive or issue entry) as an `unattributed_commits` list (sha, date, subject) in JSON and an `Other changes` footer in markdown, so downstream consumers (the stylist, an operator) see that work shipped even when it bypassed both lanes — instead of asserting nothing happened.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `orchestrator-cli`: the "`changelog` subcommand harvests changelog entries from the OpenSpec archive" requirement gains corrected default-`since` resolution, a degenerate-range error, issues-lane harvesting, and the unattributed-commit sweep.

## Impact

- `autocoder/src/cli/changelog.rs`: `resolve_tag_range` (default-since exclusion + equal-range error), `find_archives_in_range` (issues lane), new unattributed-commit walk, markdown/JSON rendering.
- `autocoder/src/changelog_triage.rs`: section JSON handed to the stylist carries the new fields; the chatops explicit-range path inherits the fixed resolution for free (it calls the same resolver).
- `prompts/changelog-stylist.md`: instruct the stylist to fold issue entries and classify unattributed commits (bugfix/docs/internal), and to write "maintenance release" ONLY when entries, fixes, and unattributed commits are all empty.
- JSON consumers see two additive fields (`lane` on entries, `unattributed_commits` per section); existing fields are unchanged.
