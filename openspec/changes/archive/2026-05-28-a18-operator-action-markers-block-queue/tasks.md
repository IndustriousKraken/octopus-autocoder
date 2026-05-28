## 1. Queue-blocking extension

- [x] 1.1 Locate the polling loop's marker-check that gates queue walks (today checks `.in-progress*` AND `.needs-spec-revision.json`).
- [x] 1.2 Extend the check to ALSO halt on `.perma-stuck.json` in any change directory.
- [x] 1.3 The halt rule: if any change in `openspec/changes/<slug>/` has at least one of `{.in-progress*, .needs-spec-revision.json, .perma-stuck.json}` AND does NOT have `.ignore-for-queue.json`, the entire queue walk for this repo halts for the iteration.
- [x] 1.4 The existing per-marker behaviors (perma-stuck change excluded from `list_pending`, needs-spec-revision halts iteration) remain. The change is the queue-blocking scope.
- [x] 1.5 Tests:
  - Workspace with one `.perma-stuck.json` change + one pending change without markers → queue halts; pending change NOT processed.
  - Same workspace with `.ignore-for-queue.json` added to the perma-stuck change → queue resumes; pending change IS processed.
  - Workspace with one `.needs-spec-revision.json` change + one pending → queue halts (unchanged behavior).
  - Workspace with no operator-action markers → queue proceeds normally.

## 2. `.ignore-for-queue.json` schema

- [x] 2.1 New file format at `<workspace>/openspec/changes/<change>/.ignore-for-queue.json`. Schema:
  ```json
  {
    "change": "<change-name>",
    "marked_at": "2026-05-27T20:30:00Z",
    "marked_by": "<operator-identifier>",  // e.g. Slack user id from the chatops command
    "reason": "operator-driven skip; original marker(s) preserved",
    "operator_action": "Delete this file (or use @<bot> clear-ignore) to re-block the queue on the original marker."
  }
  ```
- [x] 2.2 The file is intentionally git-tracked AND committed (consistent with `.perma-stuck.json` AND `.needs-spec-revision.json` treatment). It survives `wipe-workspace` via the re-clone.
- [x] 2.3 Tests: serialize + deserialize round-trip; missing fields produce sensible defaults.

## 3. Chatops verb dispatch

- [x] 3.1 Add `ignore-and-continue` AND `clear-ignore` to the inbound verb table (likely `autocoder/src/chatops/slack.rs` or wherever verbs are parsed). Both verbs take `<repo-substring> <change-slug>` arguments.
- [x] 3.2 The dispatcher resolves the repo + change via the same substring-matching used by `clear-perma-stuck` / `clear-revision`.
- [x] 3.3 `ignore-and-continue` writes the marker file AND commits/pushes the change directory's update. The commit subject is `chore: ignore-for-queue on <change> (operator <id>)`. Push uses the daemon's normal push path.
- [x] 3.4 `clear-ignore` removes the marker file AND commits/pushes the removal. Subject: `chore: clear ignore-for-queue on <change>`.
- [x] 3.5 Reply shapes:
  - Success: `✓ Marked <change> as ignored for queue. Subsequent changes will process; <change> stays excluded until the underlying marker is cleared.`
  - Already marked: `✗ <change> already has .ignore-for-queue.json. No change.`
  - No underlying marker: `✗ <change> has no operator-action marker (perma-stuck OR needs-spec-revision). Ignore is a no-op; rejecting to prevent confusion.`
  - Symmetric refusals for `clear-ignore`.
- [x] 3.6 Tests: each happy + refusal path against `RecordingActions`.

## 4. Status reply annotation

- [x] 4.1 In the status-reply composer, when scanning the workspace's `openspec/changes/*/` directories for active markers, also check for `.ignore-for-queue.json` alongside each blocking marker found.
- [x] 4.2 When a blocking marker is paired with `.ignore-for-queue.json`, the "active markers" line for that change gains the annotation `(ignore-for-queue: yes — queue not blocked)`. When unaccompanied, no annotation.
- [x] 4.3 Tests: status output for various marker combinations matches the expected text.

## 5. Help-verb extension

- [x] 5.1 `@<bot> help` reply gains `ignore-and-continue` AND `clear-ignore` entries in the verb list. One-line descriptions consistent with the others.

## 6. Docs

- [x] 6.1 In `docs/OPERATIONS.md`'s "Perma-stuck change detection" section, add a paragraph naming the new queue-blocking behavior AND the ignore-and-continue escape hatch. Cross-link to CHATOPS.md for the verb syntax.
- [x] 6.2 In `docs/CHATOPS.md`'s operator-recovery-commands section, add rows for `ignore-and-continue` AND `clear-ignore`. Include example reply shapes.
- [x] 6.3 In `docs/OPERATIONS.md`'s queue-blocking-policy discussion (if extant; create the section if not), enumerate the FOUR marker categories that block the queue AND note that `.ignore-for-queue.json` downgrades any of them.

## 7. Spec deltas

- [x] 7.1 `openspec/changes/a18-operator-action-markers-block-queue/specs/orchestrator-cli/spec.md` MODIFIES `Perma-stuck change detection` (preserving all 6 existing scenarios) AND ADDs `Ignore-for-queue marker downgrades blocking-marker behavior without unblocking the change itself`.
- [x] 7.2 `openspec/changes/a18-operator-action-markers-block-queue/specs/chatops-manager/spec.md` MODIFIES the operator-verbs requirement to add the two new verbs (preserving all existing verb scenarios) AND MODIFIES the status-reply-annotation requirement (preserving all existing scenarios) to add the ignore-for-queue annotation case.
- [x] 7.3 `openspec/changes/a18-operator-action-markers-block-queue/specs/project-documentation/spec.md` ADDs `OPERATIONS.md AND CHATOPS.md document the queue-blocking change AND the ignore verbs`.

## 8. Verification

- [x] 8.1 `cargo test` passes (new + existing).
- [x] 8.2 `openspec validate a18-operator-action-markers-block-queue --strict` passes.
- [x] 8.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
