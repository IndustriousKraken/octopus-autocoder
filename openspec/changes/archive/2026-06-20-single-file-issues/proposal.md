# Single-file issues

## Why

An issue is the issues lane's unit of behavior-preserving correction. It is
currently always a directory `issues/<slug>/` holding `issue.md` + `tasks.md`,
mirroring an OpenSpec change's multi-artifact layout. But an issue is autocoder's
own lightweight construct, not an OpenSpec change — most issues are small (a
short diagnosis and a few fix steps), and a directory-plus-two-files for a
three-line fix is more ceremony than the unit warrants. A single file with a
description and a short task checklist is easier to author, read, and scan, and
keeps the `issues/` tree shallow.

The one thing a single file cannot safely absorb is an untrusted public report
body: the a010 ingestion path keeps that body in a separate `report-body.md`,
quarantined as DATA distinct from the maintainer-approved task, and the
implementer prompt relies on that separation. Merging the body into one file with
the task would dissolve that boundary. So the unit stays a directory exactly when
it needs to — which maps cleanly to the trust/complexity boundary.

## What Changes

- An issue MAY be a **single file** `issues/<slug>.md` (a description plus an
  optional `## Tasks` checklist) — the default form for curated, simple
  corrections — OR a **directory** `issues/<slug>/` (`issue.md` + `tasks.md`) when
  it must carry a separate artifact. Neither form carries a `specs/` directory.
- A **public-origin** issue (untrusted body) SHALL use the directory form so the
  quarantined `report-body.md` stays a separate file from the task. The single-
  file form is for curated/trusted issues only.
- The lane's loader, ready-list, lock, archive, and dedup accept BOTH forms. A
  single-file issue's per-issue markers are sibling files (`issues/<slug>.<marker>`);
  a directory issue's markers stay inside the directory. The ready-list treats an
  `<slug>.md` file or an `<slug>/` directory as a unit and ignores marker siblings.
- Completion archives a single-file issue to `issues/archive/<date>-<slug>.md`
  and a directory issue to `issues/archive/<date>-<slug>/` — no canon touched.
- The issue-flavored implementer reads the unit's body (the single file, or
  `issue.md`); the reviewer's issue brief reads the same. No behavior change to
  triage, dedup, classification, precedence, or the quarantine of public bodies.

## Impact

- Affected specs: `orchestrator-cli` (MODIFY **Issues lane for corrections** — the
  two-form unit shape, marker placement, archive paths, and the public-origin
  directory requirement; AND MODIFY **Issues lane parks a non-progressing issue** —
  the park marker is a sibling for a single-file issue, AND the git-exclude set
  covers that sibling form via a suffix pattern, not only the in-directory name;
  AND MODIFY **Hybrid issue ingestion with maintainer promotion** — the promotion
  control-socket action writes the form appropriate to the candidate's origin
  (single file for a curated candidate, directory for a public-origin candidate)).
- Affected code: `lanes/issues.rs` (loader, `list_ready`, `lock`/`unlock`,
  `archive`, `issue_dir` resolution, `is_perma_stuck`/marker helpers accept a file
  OR a directory; markers as siblings for the file form, archive via a file-capable
  dated move), `lanes/ingestion.rs` (curated candidate may draft
  a single file; public-origin candidate keeps the directory form),
  `workspace.rs` (register `*.perma-stuck.json`/`*.in-progress` excludes so the
  sibling markers are gitignored), `rollback.rs` (`resolve_units`/`unarchive_issue`
  recognize a single-file archived issue), `polling_loop/review_context.rs` (the
  issue brief reads the unit's body), and the issue-flavored implementer prompt
  (reads the unit's body).
- The a010 quarantine requirement is unchanged: public-origin issues keep the
  directory form with a separate `report-body.md`.
