## Context

`resolve_tag_range` (`autocoder/src/cli/changelog.rs:160`) defaults `since` via `git describe --tags --abbrev=0 <to>` — which, when `--to` names a tag, returns that tag itself, producing the empty range `(vX .. vX]`. The gap-fill path already documents this footgun and guards its own call site (`changelog_triage.rs:372-377`); the explicit-args path and the bare CLI do not. Observed in production: autocoder's own v1.3.1 changelog PR (#171) claimed "no changelog-tracked changes" for a window that held five archived changes and four issues-lane security fixes. Separately, `find_archives_in_range` walks only `openspec/changes/archive/`, so issues-lane corrections and out-of-lane commits never reach the changelog at all — a repo whose release was mostly bugfixes reports as empty.

## Goals / Non-Goals

**Goals:**
- Make `--to <tag>` without `--since` document the release ending at that tag.
- Make a degenerate `since == to` range a loud error instead of a quiet empty document.
- Surface issues-lane corrections as first-class changelog entries.
- Report out-of-lane commits so "nothing shipped" is only ever said when nothing shipped.

**Non-Goals:**
- Classifying unattributed commits (bugfix vs docs vs internal) — that is judgment, which belongs to the stylist LLM or the operator, not the deterministic extractor.
- Rebuild semantics (regenerating already-documented sections) — separate change (`changelog-rebuild-regenerates-all-sections`).
- Changing which PRs/branches the chatops flow opens — this change is extractor-level.

## Decisions

- **Default-since exclusion, not `<to>^` arithmetic.** Resolve the default via `git tag --points-at <to-commit>` and pass each result as `--exclude <tag>` to `git describe --tags --abbrev=0 <to-commit>`. This excludes exactly "tags naming the `to` commit" and keeps every other describe behavior; `<to>^`-style parent walking mis-handles merge commits and tags on other ancestry lines.
- **`since == to` is an error even when explicit.** An operator who typed the same ref twice made a mistake; a silent empty document turns that mistake into a wrong changelog. The error names both refs and the shared commit. The gap-fill caller never produces equal ranges (its lower bound is always a *different* tag or `ever`), so the guard does not disturb flagless runs.
- **Issue summaries come from `issue.md` first body paragraph.** Issues have no `## Why` convention; the first paragraph after any leading heading is the closest equivalent. The `changelog:` frontmatter overrides apply identically, so operators can curate issues the same way they curate changes.
- **`lane` tag + `Fixes` group instead of a parallel entries array.** One entries list with a `lane` discriminator keeps the JSON shape additive (existing consumers ignore the new field); markdown groups issues under `### Fixes` because issues have no owning capability.
- **Unattributed sweep = range commits minus lane-adding commits, merges excluded.** Implemented as one `git log --no-merges --format=%H%x09%ad%x09%s` walk plus the already-computed set of lane-adding commit SHAs. Merge commits are excluded because their subjects duplicate the branch content. The extractor reports; the stylist prompt (updated in this change) folds them into prose or omits trivia, and writes "maintenance release" only when entries, fixes, AND unattributed commits are all empty.

## Risks / Trade-offs

- [Noisy `unattributed_commits` on repos with chatty direct-commit habits] → It's a report of reality; the stylist is instructed to summarize/omit trivia, and `--format json` consumers can ignore the field. Better noisy-true than quiet-false.
- [`git tag --points-at` adds one subprocess per run] → Negligible; the extractor already shells several git commands.
- [Existing JSON consumers that `deny_unknown_fields`] → The only consumer is the stylist payload builder in this codebase; it is updated in the same change.
