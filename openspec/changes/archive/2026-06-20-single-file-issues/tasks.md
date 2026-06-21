# Tasks

## 1. Loader + lifecycle accept both forms

- [x] 1.1 In `lanes/issues.rs`, make issue resolution accept a single file `issues/<slug>.md` OR a directory `issues/<slug>/`. `load` reads the body from the file (single-file) or from `issue.md` (directory); a `## Tasks` section in the single file is the task list. A `specs/` directory under a directory-form issue is still `MalformedHasSpecsDir`.
- [x] 1.2 `list_ready` treats a top-level `<slug>.md` file OR a non-`archive`, non-`.`-prefixed `<slug>/` directory as a unit. It SHALL skip the issues lane's own marker siblings (`<slug>.in-progress`, `<slug>.perma-stuck.json`) and any other non-`.md`, non-directory sibling. (The issues lane writes only those two markers; `.ignore-for-queue.json`/`.needs-spec-revision.json` are changes-lane markers and never appear for an issue.) A single-file slug is the filename minus `.md`.
- [x] 1.3 Per-issue marker helpers resolve to siblings for a single-file issue (`issues/<slug>.<marker>`) and inside the directory for a directory issue. The `.in-progress` lock (`lock`/`unlock`) AND the perma-stuck park check (`is_perma_stuck`/`write_perma_stuck`) follow the same rule, so `list_ready` skips a parked single-file issue via its sibling `.perma-stuck.json` exactly as it skips a parked directory issue.
- [x] 1.4 `archive` moves `issues/<slug>.md` → `issues/archive/<UTC-date>-<slug>.md` and `issues/<slug>/` → `issues/archive/<UTC-date>-<slug>/`. The existing shared dated-move primitive (`shared::archive_dir_with_postcondition`) is directory-only (it asserts `is_dir()` on the source AND the destination postcondition), so the file form needs a file-capable dated move — a sibling helper, OR a generalization of the shared primitive that handles a file unit with a file postcondition; the directory form keeps using the existing primitive. Transient marker siblings (`.in-progress`) are dropped, not archived; the unit's body is the self-contained archive entry.
- [x] 1.5 Dedup (`existing_issue_slugs`) derives the slug from BOTH a `<slug>.md` file and a `<slug>/` directory, in `issues/` and `issues/archive/`, so a re-report is deduped regardless of form. The same file-aware enumeration applies uniformly to every enumerator that currently assumes the directory form (`list_ready`, `existing_issue_slugs`, `is_malformed`/`load`, `is_perma_stuck`/marker resolution) — not dedup alone.
- [x] 1.6 Register git-exclude patterns that cover a single-file issue's SIBLING markers (`*.perma-stuck.json`, `*.in-progress`) at workspace init (`workspace.rs`, next to the existing marker excludes). The current registration uses bare basenames (e.g. `.perma-stuck.json`), which git matches only when the WHOLE basename equals the pattern — so `issues/<slug>.perma-stuck.json` would NOT be ignored, would trip the pre-pass dirty check, AND would be wiped by `git clean`. The suffix patterns match both the in-directory and sibling forms at any depth.
- [x] 1.7 Make code-rollback recovery unarchive a single-file archived issue. `rollback::resolve_units` and `unarchive_issue` are directory-only (they key the slug off the first path segment under `issues/archive/` AND assert `src.is_dir()`, renaming to a directory). For an `issues/archive/<UTC-date>-<slug>.md` entry, strip BOTH the date prefix AND the `.md` suffix to recover `<slug>`, AND move the file back to `issues/<slug>.md`. The canon rollback requirement already speaks of the "issue unit" generically, so this is a code reconciliation, not a new requirement.

## 2. Ingestion / candidate drafting

- [x] 2.1 A curated candidate MAY be drafted as a single file `issues/<slug>.md` (description + `## Tasks`). A public-origin candidate (carrying an untrusted body) SHALL be drafted as the directory form `issues/<slug>/` with the quarantined `report-body.md` separate, per the unchanged quarantine requirement — never a single file.
- [x] 2.2 The promotion control-socket action writes the appropriate form for the candidate's origin (single file for curated, directory for public-origin).

## 3. Implementer + reviewer read the unit body

- [x] 3.1 The issue-flavored implementer reads the unit's body from the single file (or `issue.md`); a `## Tasks` checklist in the single file is the task list. No change to the canon-verification / contract-change behavior.
- [x] 3.2 `polling_loop/review_context.rs`: the issue brief (a009 reviewer context) reads the single file's body, or `issue.md`/`tasks.md` for the directory form, locating the archived unit as either `issues/archive/<date>-<slug>.md` or `issues/archive/<date>-<slug>/`.

## 4. Tests

- [x] 4.1 A single-file `issues/<slug>.md` (description + `## Tasks`) loads, is listed ready, is worked, and archives to `issues/archive/<date>-<slug>.md`.
- [x] 4.2 A directory-form issue still loads/works/archives to `issues/archive/<date>-<slug>/` (regression); a directory with `specs/` is rejected as malformed.
- [x] 4.3 A public-origin issue is written in the directory form with a separate `report-body.md`; a single-file form is never produced for a public-origin candidate.
- [x] 4.4 Marker placement: a single-file issue's lock/perma-stuck markers are siblings AND are NOT mistaken for units by `list_ready` (a parked single-file issue is skipped via its sibling `.perma-stuck.json`); the sibling markers are covered by the registered suffix excludes (gitignored) AND do not appear in the pre-pass dirty check; a directory issue's markers stay inside.
- [x] 4.5 Dedup recognizes a prior issue in either form (file or directory), in active and archived locations.
- [x] 4.6 Code-rollback recovery unarchives a single-file archived issue (`issues/archive/<date>-<slug>.md`) back to `issues/<slug>.md` (regression: a directory-form archived issue still unarchives to `issues/<slug>/`).
