# Tasks

## 1. Reviewer accepts a diff-or-target surface

- [ ] 1.1 Extend `ReviewContext` (and the agentic prompt renderer) so the review surface is EITHER a unified diff + changed-file paths (pass / PR / commit) OR a target file-path list + operator focus with no diff. The diff-based rendering is unchanged; the no-diff target rendering carries the focus + file list in place of the diff. Reads-on-demand, sandbox, and `submit_review` are unchanged.

## 2. Target resolution

- [ ] 2.1 `pr <N>`: resolve the PR's base..head range from the local clone and produce its diff (forge supplies number→SHA; `git diff`).
- [ ] 2.2 `commit <sha>`: produce the commit's diff (`git show <sha>`).
- [ ] 2.3 `files <path...>`: build a target file-set (no diff).
- [ ] 2.4 free-text description: build a target whose surface is the description; the reviewer locates files via `Glob`/`Grep` during the session. The report names the files actually reviewed.

## 3. Operator surface

- [ ] 3.1 Add a `review` chatops verb (`@<bot> review <repo-substring> <target>`) and a CLI subcommand, using the existing repo-selector resolution. Dispatch via a control-socket action.
- [ ] 3.2 Report the verdict + concerns to the originating chat channel; for a `pr` target, optionally post the review as a PR comment. The command is advisory/read-only — no revision, no code/marker change.
- [ ] 3.3 A session that produces no valid verdict surfaces the failure (gatekeepers-fail-closed), not a clean pass.

## 4. Scale: chunk-and-aggregate

- [ ] 4.1 When the target spans more files than one bounded session, run multiple reviewer sessions over chunks (per file or per module) and aggregate findings into one report; log what was chunked. A bounded target uses a single session.

## 5. Tests

- [ ] 5.1 `pr`/`commit` targets resolve to the right diff and run the reviewer; the verdict is reported.
- [ ] 5.2 A `files` target runs a no-diff target review (assert the surface carries the file list, not a diff).
- [ ] 5.3 A description target reviews agent-located files and the report names them.
- [ ] 5.4 A large target is split into multiple sessions and aggregated (assert >1 session, one report).
- [ ] 5.5 The command opens no revision and changes no code/marker; a no-verdict session surfaces failure.
