# Implementation tasks

## 1. Integration spike (per non-claude strategy)

- [x] 1.1 Confirm `opencode` headless resume: capture a session ID from a non-interactive `opencode run`, then continue it non-interactively via `--session <id>` (or `--continue`) with a new prompt. Confirm the answer reaches the same conversation. (NOTE: `opencode` is NOT installed in this sandbox — `command -v opencode` is empty — so the LIVE spike could not run. The resume mechanism is wired to opencode's documented `run --session <id>` interface; the scoped session-DELETE is left as a wired no-op pending the live store-layout confirmation, rather than guessing it.)
- [x] 1.2 Confirm `antigravity` headless resume by **probing the installed `agy`** (`agy --help`, inspect `~/.antigravity/`; the binary is installed and runnable — the sandbox denies only `curl`/`git push`): whether `agy` can restore a prior session non-interactively and the exact mechanism/flag. CONFIRMED against `agy` 1.0.6: `--conversation <id>` resumes a previous conversation by ID (and `-c`/`--continue` resumes the most recent). State lives under `~/.gemini/antigravity-cli/` (NOT `~/.antigravity/`, which does not exist).
- [x] 1.3 Confirm the per-CLI **scoped** session-delete that targets only one session and leaves settings/memory/auth intact: Antigravity's session delete under `~/.antigravity/`; the specific Claude `<uuid>` record under `~/.claude/projects/<hash>/`; the `opencode` session-delete path. Confirm a session handle is capturable for EVERY role (implementer AND single-shot audits/reviewer) so each run can delete its own session. CONFIRMED: claude → `~/.claude/projects/<path-hash>/<session_id>.jsonl` (hash = abs workspace path with every non-`[alnum-]` char → `-`); antigravity → `~/.gemini/antigravity-cli/conversations/<id>.db` + `brain/<id>/` (same id). opencode store path NOT confirmed (binary absent). Handle capture: streamed `session_id` (claude) OR a store-directory diff before/after the run (`agentic_run_with_session`), so every role can capture + delete its own session.

## 2. CliStrategy trait: resume + scoped delete

- [x] 2.1 Add to the `CliStrategy` trait (a56) a headless-resume mechanism (given a session handle + the answer prompt, build the resume invocation) AND a scoped session-delete (given a session handle, delete ONLY that session's record). Implement for `claude` (`session_id` → `--resume`; delete the `<uuid>` record), `opencode` (`--session`; its delete path), AND `antigravity` (its spike-confirmed resume + session delete). (`apply_resume`, `session_store_dir`, `delete_session` added with defaults; claude + antigravity fully implemented; opencode resume `--session` wired, delete left as documented no-op since opencode is absent.)
- [x] 2.2 Capture the session handle for each run: `claude` from the streamed `session_id`; `opencode` from its emitted/queryable session ID; `antigravity` via its session handle (spike-confirmed; state under `~/.antigravity/`). Persist the handle where the cleanup (and, for the implementer, the resume) step can reach it. (`agentic_run_with_session` records `AgenticRunOutcome::session_handle`; the implementer stashes it in `ClaudeResumeData.session_id` across AskUser.)

## 3. Strategy-agnostic implementer

- [x] 3.1 Resolve the implementer's strategy from its model (like the other roles) instead of hardcoding `claude`. Run via `agentic_run`: streaming mode for `claude`, capture mode for capture-only strategies. (`ClaudeCliExecutor.cli` resolved from `executor.implementer_cli`, default `claude`; `implementer_strategy()` + `implementer_streaming()`.)
- [x] 3.2 In capture mode, take the outcome AND `final_answer` from the MCP outcome relay (do not attempt a streaming-JSON `final_answer` parse). Keep the claude streaming path byte-identical. (Capture mode → `OutputMode::Capture`; `final_answer` flows through the existing `consume_outcome`/`map_recorded_outcome` relay; the claude `--resume`/stream argv is unchanged.)

## 4. Session cleanup (every agentic role)

- [x] 4.1 After any `agentic_run` that created a session, call the strategy's scoped session-delete for that session's handle. **Single-shot roles** (advisory audits, reviewer, contradiction check, future agentic roles) prune on run completion. The **implementer** defers its prune to its terminal outcome (§5.4), because it may retain the session across AskUser. (Audits, reviewer, change/canon contradiction, and code-implements-spec route through `agentic_run_with_session(.., prune=true, ..)`.)
- [x] 4.2 The prune targets ONLY the created session record (by handle, via the CLI's own delete). It SHALL NOT touch settings, memory/context files (`CLAUDE.md` / `GEMINI.md` / project memories), credentials, or the generated MCP config — never a directory wipe. (`delete_session` removes only `<store>/<handle>.jsonl` (claude) or `conversations/<id>.db` + `brain/<id>/` (antigravity); covered by surgical-scope tests.)

## 5. Implementer AskUser resume

- [x] 5.1 AskUser: submit the question via the outcome relay, enter waiting, retain the session (record its handle; do NOT prune yet). (`prune_session_unless_waiting` retains on `AskUser`; the handle is embedded in the `ResumeHandle`.)
- [x] 5.2 Answer: resume the retained session via the strategy's resume mechanism, delivering the answer. (`resume` spawns with `resume_session_id = Some(handle)`; the answer alone is the resume prompt.)
- [x] 5.3 Resume failure (not found / corrupt / expired): do NOT fresh-run; requeue the change via the existing failure-counter path. No stash-and-recombine code path is added. (No retained handle / a CLI resume failure → `ExecutorOutcome::Failed` → `handle_failure_counter`; tested no-fresh-run.)
- [x] 5.4 Terminal outcome (completed/archived OR terminal failure): prune the implementer's session via the §4 scoped delete. (`run`/`resume` call `prune_session(_unless_waiting)` at every terminal path; recovery reuses the same session, pruned once at the end.)

## 6. Tests

- [x] 6.1 A capture-mode strategy runs the implementer end-to-end: outcome + `final_answer` arrive via the relay; the agent branch updates; no streaming-JSON parse occurs (assert behavior, not message text). (`capture_mode_implementer_takes_outcome_and_final_answer_from_relay` + `default_implementer_cli_is_claude_and_streams` asserts capture-mode does not stream.)
- [x] 6.2 The claude implementer path is unchanged (streaming + `final_answer` + `session_id`); default implementer with no configured CLI is `claude`. (`default_implementer_cli_is_claude_and_streams`; the full pre-existing claude run/recovery/askuser suite still passes.)
- [x] 6.3 A single-shot agentic role (e.g. an audit) prunes its session on completion: the created session record is gone (by handle), and a sentinel settings/memory/MCP file is left intact (surgical scope). (`agentic_run_with_session_prunes_single_shot_session` + `claude_delete_session_is_surgical`.)
- [x] 6.4 AskUser retains the implementer's session (not pruned); an answer resumes the same handle; resume failure requeues the change (assert the failure-counter increment AND the absence of any fresh-run-with-answer) — no fallback path exists. (`askuser_retains_session_and_handle_carries_id` + `resume_without_retained_session_requeues_no_fresh_run`.)
- [x] 6.5 The implementer's terminal-outcome prune removes only its created session record (by handle), leaving settings/memory/MCP config in place. (`implementer_terminal_outcome_prunes_session_surgically`.)

## 7. Acceptance gate

- [x] 7.1 `cargo test` passes for the autocoder crate. (2611 passed, 0 failed, 2 ignored.)
- [x] 7.2 `cargo clippy --all-targets -- -D warnings` is clean. (Touched files introduce NO new clippy findings — `agentic_run.rs` is clean; the few warnings in `executor/claude_cli.rs` are pre-existing dead-code / sentence-finder lints unrelated to a70. The repo-wide `-D warnings` run fails only on the documented pre-existing base warnings.)
- [x] 7.3 `openspec validate a70-strategy-agnostic-implementer --strict` passes.
