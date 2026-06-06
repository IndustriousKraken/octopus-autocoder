## Why

Not all work is a spec change. There are two kinds:

- **Spec changes** — new or changed behavior. They belong in `changes/` and carry a spec delta.
- **Corrections** — fixes to code that is *already correctly specified* (bug fixes, behavior-preserving refactors). They change no spec, so they have no delta. Today they must be forced into `changes/` with a manufactured `project-documentation` requirement (as `a68` was), because OpenSpec rejects a delta-less change.

This change gives corrections a first-class home: an `issues/` lane that verifies a fix against the **existing canon** instead of a delta. The verification asymmetry is the load-bearing distinction — a change asks "does the code match the new delta?", an issue asks "does the fix make the code match the spec it already has?" Both are verifiable predicates, and the issue path needs no new spec, so the existing verifier/reviewer machinery applies, pointed at canon.

This phase ships the **lane mechanism + the curated entry path** (a maintainer commits `issues/<slug>/` directly). Public-issue ingestion and the prompt-quarantine trust boundary are `a010`, stacked on this.

## What Changes

**A second work lane, `issues/`.** An issue is `issues/<slug>/` containing `issue.md` (the report/diagnosis + acceptance criteria stated against the existing spec) and `tasks.md` (the fix steps), with **no `specs/` directory** — that absence is the contract that an issue changes no spec. The lane is gated by a `features.issues` flag, off by default. The curated entry path is a maintainer committing the directory directly (repo write is the allowlist). On completion the directory moves to `issues/archive/`, mirroring `changes/archive/`; the canonical spec is not modified (audit trail only).

**Two independent walkers over shared utilities.** The changes lane and the issues lane are driven by **separate walkers**, each with its own control flow and its own state file — not one walker with an `is_issue` flag. Shared leaf functionality (busy-marker, PR opening, archiving, chatops notify, queue-state I/O, workspace) is extracted into stateless utilities both walkers compose. A fault in one walker cannot corrupt the other lane's control flow or state.

**Lane precedence: `issues > changes > audits`.** Within the existing per-repo serializer (the busy-marker — one unit at a time), each iteration picks the highest-precedence ready unit, extending the established changes-over-audits order. Within a lane, selection is alphabetical. Issue-precedence is strict; anti-starvation comes from the promotion gate (`a010`), not a fairness rule.

**Issue-flavored implementer prompt.** Running an issue uses a prompt that says: fix the code to match the existing spec; do not invent a spec change; if the fix actually needs changed behavior, kick it back to `changes/`. Acceptance is verified against the existing canon.

## Impact

- **Affected specs:** `orchestrator-cli` — ADD `Issues lane for corrections`, `Independent lane walkers over shared utilities`, `Lane precedence — issues over changes over audits`. `executor` — ADD `Issue-flavored implementer prompt verifies against existing canon`.
- **Affected code:** a shared-utility module (extract busy-marker / open_pr / archive / chatops_notify / queue-state I/O / workspace, behavior-preserving for the changes walker); a new issues walker with its own state file; the polling iteration's unit selection extended to `issues > changes > audits` (alphabetical within a lane); `issues/<slug>/` loading + `issues/archive/` move + the malformed-`specs/`-dir rejection; a `features.issues` config flag; the issue-flavored implementer prompt (loaded through the uniform PromptLoader, override field per the nested naming convention).
- **Operator-visible behavior:** with `features.issues` on, a committed `issues/<slug>/` is selected ahead of changes and audits, fixed against the existing spec, PR'd, and on completion archived to `issues/archive/` without modifying any spec. Corrections no longer need a manufactured delta. Off by default.
- **Dependencies:** independent (`a000`/`a002` archived). `a010` (hybrid ingestion + quarantine) stacks on this. Drift handling is light: because issues run first, a change may later find its work already done — a plain failure for that change is the acceptable outcome; no rebase-precheck in v1.
- **Acceptance:** `cargo test` passes; `cargo clippy --all-targets -- -D warnings` is clean; `openspec validate a009-issues-lane-curated --strict` passes. Tests: an `issues/<slug>/` with a `specs/` directory is rejected; a ready issue is selected before a ready change, and a change before an audit; the two walkers read/write only their own state file; completion moves the directory to `issues/archive/` without modifying any canonical spec; an issue run uses the issue-flavored prompt and reports a behavior-change fix back to `changes/`.
