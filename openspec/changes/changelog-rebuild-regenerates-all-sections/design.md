## Context

Flagless gap-fill (`build_stylist_payload` in `autocoder/src/changelog_triage.rs:343`) is deliberately idempotent: it documents only stable tags missing from `CHANGELOG.md`. That is right for routine runs but leaves no repair path when previously-generated sections are wrong — which every pre-`changelog-extractor-accuracy` section potentially is (empty "maintenance release" sections for windows that shipped security fixes, missing issues-lane entries). The only current remedies are hand-editing or deleting `CHANGELOG.md` so gap-fill regenerates from scratch, both of which lose the review-PR workflow's safety.

## Goals / Non-Goals

**Goals:**
- One verb argument (`--rebuild`) that regenerates every stable-tag section under current extractor coverage, reviewed as a single PR.

**Non-Goals:**
- A CLI-level `--rebuild` on the pure extractor — the extractor is single-range by design; rebuild is a multi-section stylist operation and lives at the verb/triage layer.
- Preserving hand-edits inside regenerated version sections — replacing them is the point; the PR review is the guard. (Preamble and `## [Unreleased]` are preserved because they are not version sections.)
- Automatic detection that a rebuild is needed.

## Decisions

- **Rebuild = gap-fill with the documented-versions filter dropped.** `build_stylist_payload` already computes stable tags and per-tag ranges; `--rebuild` skips the `documented` subtraction and marks the payload (`"mode": "rebuild"`) so the stylist knows to replace sections in place. Smallest possible diff on the existing flow.
- **Mutually exclusive with `--since`/`--to`.** A partial rebuild is just the existing explicit-range run; allowing the combination would create two ways to say the same thing plus an ambiguous third (rebuild-within-range). Refuse politely, before any state file is written, matching the other argument refusals.
- **Section replacement is the stylist's job, guarded by the existing path-scope validation.** The daemon does not splice markdown; it already delegates insertion positioning to the stylist, and replacement is the same class of edit. The diff still may only touch `CHANGELOG.md` (and `changelog:` frontmatter files), so a misbehaving stylist cannot escape scope.
- **NoOp only on "no stable tags".** Unlike flagless runs, "everything documented" is not a no-op for rebuild — regenerating documented sections is the request.

## Risks / Trade-offs

- [Rebuild PR diffs are large on long-lived repos] → Inherent to the request; oldest-first ordering and one-section-per-version structure keep the diff reviewable. Operators can still `@<bot> revise`.
- [Stylist could duplicate instead of replace a section] → The payload's rebuild marker plus prompt guidance address it; the operator reviews the diff, and the revision loop fixes stragglers.
- [Hand-curated wording in old sections is lost] → Stated in the verb's reply and this design; curation that must survive belongs in `changelog:` frontmatter overrides, which regeneration honors.
