# Tasks

## 1. Loader + lifecycle accept both forms

- [ ] 1.1 In `lanes/issues.rs`, make issue resolution accept a single file `issues/<slug>.md` OR a directory `issues/<slug>/`. `load` reads the body from the file (single-file) or from `issue.md` (directory); a `## Tasks` section in the single file is the task list. A `specs/` directory under a directory-form issue is still `MalformedHasSpecsDir`.
- [ ] 1.2 `list_ready` treats a top-level `<slug>.md` file OR a non-`archive`, non-`.`-prefixed `<slug>/` directory as a unit. It SHALL skip marker siblings (`<slug>.in-progress`, `<slug>.perma-stuck.json`, `<slug>.ignore-for-queue.json`, `<slug>.needs-spec-revision.json`) and any other non-`.md` file. A single-file slug is the filename minus `.md`.
- [ ] 1.3 Per-issue marker helpers resolve to siblings for a single-file issue (`issues/<slug>.<marker>`) and inside the directory for a directory issue. The `.in-progress` lock (`lock`/`unlock`) follows the same rule.
- [ ] 1.4 `archive` moves `issues/<slug>.md` → `issues/archive/<UTC-date>-<slug>.md` and `issues/<slug>/` → `issues/archive/<UTC-date>-<slug>/` via the shared dated-move primitive. Move marker siblings with the unit (or drop transient ones) so the archive entry is self-contained.
- [ ] 1.5 Dedup (`existing_issue_slugs`) derives the slug from BOTH a `<slug>.md` file and a `<slug>/` directory, in `issues/` and `issues/archive/`, so a re-report is deduped regardless of form.

## 2. Ingestion / candidate drafting

- [ ] 2.1 A curated candidate MAY be drafted as a single file `issues/<slug>.md` (description + `## Tasks`). A public-origin candidate (carrying an untrusted body) SHALL be drafted as the directory form `issues/<slug>/` with the quarantined `report-body.md` separate, per the unchanged quarantine requirement — never a single file.
- [ ] 2.2 The promotion control-socket action writes the appropriate form for the candidate's origin (single file for curated, directory for public-origin).

## 3. Implementer + reviewer read the unit body

- [ ] 3.1 The issue-flavored implementer reads the unit's body from the single file (or `issue.md`); a `## Tasks` checklist in the single file is the task list. No change to the canon-verification / contract-change behavior.
- [ ] 3.2 `polling_loop/review_context.rs`: the issue brief (a009 reviewer context) reads the single file's body, or `issue.md`/`tasks.md` for the directory form, locating the archived unit as either `issues/archive/<date>-<slug>.md` or `issues/archive/<date>-<slug>/`.

## 4. Tests

- [ ] 4.1 A single-file `issues/<slug>.md` (description + `## Tasks`) loads, is listed ready, is worked, and archives to `issues/archive/<date>-<slug>.md`.
- [ ] 4.2 A directory-form issue still loads/works/archives to `issues/archive/<date>-<slug>/` (regression); a directory with `specs/` is rejected as malformed.
- [ ] 4.3 A public-origin issue is written in the directory form with a separate `report-body.md`; a single-file form is never produced for a public-origin candidate.
- [ ] 4.4 Marker placement: a single-file issue's lock/perma-stuck markers are siblings AND are NOT mistaken for units by `list_ready`; a directory issue's markers stay inside.
- [ ] 4.5 Dedup recognizes a prior issue in either form (file or directory), in active and archived locations.
