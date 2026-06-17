# Tasks

## 1. Stop inlining the diff in the agentic prompt

- [ ] 1.1 In `render_agentic_review_prompt` (`code_reviewer.rs`), replace the inlined ` ```diff ... ``` ` block with a reference to the diff artifact's path: the prompt lists the change briefs, the changed-file path list, AND a line naming the diff-artifact path with an instruction to `Read` it (and to read the changed files directly for full context). Do NOT inline `ctx.diff`.
- [ ] 1.2 The oneshot prompt path is unchanged (it keeps `prompt_budget_chars`-bounded inlining). Only the agentic render path changes.

## 2. Write + clean up the diff artifact

- [ ] 2.1 Before spawning the agentic reviewer session, write the unified diff (already gathered in `build_review_context`/`PerChangeContext`) to a file the read-only sandbox can `Read` — a path inside the bound workspace that is neither committed nor surfaced as a worktree change (e.g. under the workspace's `.git/` or a dedicated dot-path), per the sandbox path-reachability constraints.
- [ ] 2.2 Remove the artifact after the session exits (success, failure, OR timeout) via an RAII guard / `finally`-style cleanup, mirroring the MCP-config cleanup already done around the session. The run must leave no diff-artifact litter.
- [ ] 2.3 For `reviewer.mode: per_change`, each per-change session references its own change-scoped diff artifact; for bundled mode, one artifact for the PR diff.

## 3. Tests

- [ ] 3.1 `render_agentic_review_prompt` output contains the changed-file path list AND a reference to the diff artifact path, AND does NOT contain the inlined diff body. (Assert on structure/derivation, not on exact prose.)
- [ ] 3.2 The prompt length is bounded independent of diff size: rendering with a tiny diff and with a very large diff produces prompts whose size differs only by the (constant-ish) artifact reference, not by the diff body.
- [ ] 3.3 The diff artifact is created before the session and removed after it (including on the timeout/error paths) — no artifact remains in the workspace after a run.
- [ ] 3.4 Regression: the agentic `ReviewResult` (verdict, per_concern, raw_output) and `reviewer.mode` dispatch are unchanged; the no-submission discard path still fails closed (no `Approve`).
