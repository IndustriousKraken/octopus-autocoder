## Why

Spec authors — humans and AI assistants alike — routinely include tasks the implementing agent (autocoder driving the Claude CLI) cannot perform inside its sandbox. Real examples observed in this very repo's recent history: "install actionlint locally", "run shellcheck", "smoke-test on a fresh Debian VM", "macOS smoke test", "push a v0.0.0-spec-smoke-test tag". The current contract has no path for the agent to signal "this task is outside what I can verify"; what happens instead is one of three undesirable shapes:

1. The agent silently skips the unimplementable tasks, leaving them as `[ ]` in tasks.md, and reports `Completed` because the other tasks WERE done. The polling loop commits and archives the change anyway (the dirty-workspace + commit + archive path doesn't validate task completion). The PR ships with unchecked tasks visible in the diff, and the operator either notices on review or doesn't. In the cases observed, the operator noticed AFTER merge and had to chat with the AI assistant to figure out what to do.

2. The agent treats the unimplementable tasks as failures, returns `Failed`. The perma-stuck counter increments; after two iterations the change is excluded. The operator gets a perma-stuck alert pointing at a vague "executor reported Failed" reason, not at "spec contains tasks I cannot do." The operator has to read the run log to figure out the underlying issue.

3. The agent attempts to do the impossible task and silently does something else (writes a placeholder, hallucinates a successful run, etc.), reports `Completed`. The PR contains fake work. This is the worst shape and has been observed in prior projects, motivating the "no stubs in changes" rule already memoized in this repo.

None of these is "the agent flags the spec as needing revision and stops, the operator gets actionable diagnosis, the spec gets fixed, the agent retries cleanly." That shape is what this change adds.

A note on the alternative — "agent auto-submits a PR proposing the spec revision": **explicitly rejected**. An agent that can edit its own marching orders to be easier is a failure mode worth avoiding. The agent flags; the operator authors the edit. Same loop as every other operator-cleared marker in this codebase (perma-stuck, archive-collision, dirty-state recovery boundaries).

## What Changes

**1. New `ExecutorOutcome::SpecNeedsRevision`.** The executor trait's outcome enum gains a variant:

```rust
pub enum ExecutorOutcome {
    Completed,
    Failed { reason: String },
    AskUser { question: String, resume_handle: ResumeHandle },
    SpecNeedsRevision {
        unimplementable_tasks: Vec<UnimplementableTask>,
        revision_suggestion: String,
    },
}

pub struct UnimplementableTask {
    /// Task identifier from tasks.md, e.g. "5.2" or "13.1".
    pub task_id: String,
    /// The literal task text, quoted from tasks.md for the alert body.
    pub task_text: String,
    /// One-line reason the task is outside the agent's sandbox.
    pub reason: String,
}
```

**2. Implementer prompt template update.** The bundled `prompts/implementer.md` (or wherever the live prompt lives) gains a section instructing the agent to scan tasks.md BEFORE starting any work. The section enumerates the kinds of tasks the agent must flag:

- Tasks requiring `sudo` against a real host (useradd, systemctl, apt install, etc.)
- Tasks requiring tools known to be absent from the sandbox (actionlint, shellcheck unless explicitly installed, jq, etc. — the agent should `command -v <tool>` to check rather than guessing)
- Tasks requiring real GitHub pushes (`git push --tags`, creating throwaway tags, etc.) — the sandbox has its own scoped credentials but not blanket org write
- Tasks requiring browser interactions (`claude auth login`, OAuth flows, etc.)
- Tasks requiring VM or container spin-up (`docker run`, `vagrant up`, etc.)
- Tasks describing "smoke tests on real hardware" or "verify on macOS" / "verify on Debian VM"
- Tasks asking for manual external observation ("confirm the deploy works in browser", "check the dashboard")

The instruction is explicit: if ANY task fits the above, the agent does NOT begin implementation. Instead, it emits the `SpecNeedsRevision` outcome with the full list and a free-form `revision_suggestion` describing what to change in tasks.md to make the spec verifiable.

**3. Outcome encoding from the Claude CLI.** The Claude CLI executor parses the agent's stdout for the outcome sentinel. The existing `AskUser` parsing pattern is the precedent — agent emits a tagged JSON block at end-of-run. `SpecNeedsRevision` uses the same mechanism with a new tag:

```
=== AUTOCODER-OUTCOME ===
{"type":"spec_needs_revision","unimplementable_tasks":[{"task_id":"5.2","task_text":"...","reason":"shellcheck not installed in sandbox; no apt access"}],"revision_suggestion":"Replace 5.2 with a CI gate via rhysd/actionlint@v1 inside the workflow itself..."}
```

The executor parses, returns the outcome to the polling loop.

**4. Marker file: `openspec/changes/<change>/.needs-spec-revision.json`.** Mirrors the `.perma-stuck.json` pattern. Schema:

```json
{
  "change": "<change-name>",
  "marked_at": "<RFC-3339 UTC timestamp>",
  "unimplementable_tasks": [
    {"task_id": "5.2", "task_text": "...", "reason": "..."},
    ...
  ],
  "revision_suggestion": "...",
  "operator_action": "Edit openspec/changes/<change>/tasks.md to remove or revise the flagged tasks, commit + push, then delete this marker file."
}
```

The marker is registered in `.git/info/exclude` at workspace init time (mirroring the `.perma-stuck.json` exclude that landed in `recover-dirty-workspace-mid-iteration`) so it doesn't trip the dirty-workspace check.

**5. `queue::list_pending` excludes changes with `.needs-spec-revision.json`.** Same logic as the existing perma-stuck exclusion — these are operator-action-required states; the change SHALL NOT be returned to the executor until the marker is gone.

**6. New `AlertCategory::SpecNeedsRevision` + chatops alert.** Posted (gated on `failure_alerts_enabled`, 24h-throttled per the existing per-category window) when the outcome fires. Alert body:

```
⚠️ `<repo>`: spec needs revision — `<change-name>` has unimplementable tasks

Tasks the agent flagged as outside its sandbox:
  - 5.2: shellcheck install (sandbox has no apt access)
  - 15.2: smoke-test on Debian VM (no host)
  - 15.3: macOS smoke test (no host)

Suggested revision:
  Replace 5.2 with a CI gate via rhysd/actionlint@v1 inside the workflow itself.
  Drop 15.2/15.3 — the workflow's own first real run is the integration test.

Operator action:
  1. Edit openspec/changes/<change-name>/tasks.md to remove or revise the flagged tasks.
  2. Commit + push to <base-branch>.
  3. Delete openspec/changes/<change-name>/.needs-spec-revision.json — the next iteration will retry the change.

marker: openspec/changes/<change-name>/.needs-spec-revision.json
log:    /tmp/autocoder/logs/<basename>/<change-name>.log
```

**7. Polling loop handles the new outcome.** In `handle_outcome` (or wherever ExecutorOutcome is dispatched), `SpecNeedsRevision` triggers: write the marker, post the alert, treat as Failed-equivalent for iteration outcome (no commit, no archive, change stays in the active path with the marker), halt the queue walk for this iteration (mirroring `halt-queue-walk-on-non-archive`).

**8. The "agent might be wrong" exit hatch.** If the operator reviews the flagged tasks and decides the agent was overly conservative, they delete the marker WITHOUT editing tasks.md. The change re-enters pending. Next iteration the agent may flag again — operator can then edit tasks.md to add a comment like `# This task IS implementable; see the README for the local-tool setup` near the flagged task, OR they may need to update the implementer prompt template if the conservatism is systemic.

**9. Operator-cleared, not auto-cleared.** Consistent with `.perma-stuck.json`'s pattern. Auto-clear on next successful iteration would muddy the operator's audit trail — "did the spec actually get fixed, or did the agent decide it wasn't a problem the second time?"

## Impact

- Affected specs: `orchestrator-cli` — one ADDED requirement "Spec-needs-revision executor outcome + marker."
- Affected code:
  - `autocoder/src/executor/mod.rs` (or wherever `ExecutorOutcome` lives) — new enum variant + struct.
  - `autocoder/src/executor/claude_cli.rs` — parser for the new sentinel format.
  - `autocoder/src/alert_state.rs` — new `AlertCategory::SpecNeedsRevision` variant.
  - `autocoder/src/queue.rs` — `list_pending` filter extension; new `is_needs_spec_revision_marked(workspace, change) -> bool` helper.
  - `autocoder/src/polling_loop.rs::handle_outcome` — dispatch arm + alert wiring + marker write.
  - `autocoder/src/workspace.rs::ensure_initialized` — add `.needs-spec-revision.json` to the `.git/info/exclude` chain.
  - `prompts/implementer.md` (or wherever the live executor prompt template is) — the new instruction section.
  - Tests in `polling_loop::tests`, `queue::tests`, `executor::claude_cli::tests`.
- Operator-visible behavior:
  - When a spec is unfixable-by-autocoder, the agent stops fast, posts a clear alert with actionable diagnosis, and the change is excluded from list_pending until the operator addresses it. No more 22-minute executor runs on doomed work; no more shipped-PR-with-unchecked-tasks confusion.
  - Operators get a new marker file class to manage. The chatops alert names the file path; the operator workflow is identical to perma-stuck (edit the underlying source, delete the marker).
- Breaking: no API or config change. The executor outcome enum is internal. Spec authors going forward should be aware that the agent will flag certain task shapes — but that's a UX improvement, not a breaking change.
- Acceptance: `cargo test` passes. `openspec validate spec-needs-revision-outcome --strict` passes. A unit test feeds the executor a tasks.md containing one obviously-unimplementable task ("run `sudo apt install actionlint` on the host") and asserts the agent returns `SpecNeedsRevision` with that task in the list AND a non-empty `revision_suggestion` AND that no commit was attempted.
