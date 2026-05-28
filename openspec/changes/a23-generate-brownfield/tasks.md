## 1. Chatops inbound parser + dispatch

- [ ] 1.1 In the chatops inbound-verb dispatch module, add `brownfield` to the recognized-verb list (alongside `propose`, `send it`, `audit`, `status`, etc.).
- [ ] 1.2 Parse `@<bot> brownfield <repo-substring> <capability-name> [optional guidance]`:
  - Resolve `<repo-substring>` via the existing case-insensitive substring-match.
  - Parse `<capability-name>` as the next whitespace-delimited token; validate against `^[a-z][a-z0-9-]*$`.
  - Treat the rest of the message as guidance (may be empty; trim; cap at 10,000 chars).
- [ ] 1.3 Refusal paths return without writing state:
  - Missing capability name → `✗ brownfield: missing capability name. Usage: @<bot> brownfield <repo> <capability-name> [optional guidance]`.
  - Slug pattern fail → `✗ brownfield: capability name must match ^[a-z][a-z0-9-]*$ (got: <name>)`.
  - Repo substring ambiguous → existing `match_repo` "be more specific" reply.
  - `features.brownfield.enabled: false` → `✗ brownfield: disabled in this workspace's config (features.brownfield.enabled=false)`.
- [ ] 1.4 At dispatch time, peek the resolved repo's workspace HEAD: if `openspec/specs/<capability-name>/spec.md` already exists, refuse with `✗ brownfield: openspec/specs/<capability-name>/spec.md already exists. Use @<bot> propose ... for changes to an existing capability.` (No state file written.)
- [ ] 1.5 On success: generate a `request_id`, post a top-level ack `✓ Queued brownfield draft for <repo_url>: capability=<capability-name>. The next polling iteration will run it (~Nm). Follow along in this thread.`; capture the ack's `ts` as `thread_ts`; write `BrownfieldRequestState { request_id, repo_url, capability_name, guidance, channel, thread_ts, status: Pending }`; submit `BrownfieldAction` over the control socket.
- [ ] 1.6 Tests: parsing happy-path (with AND without guidance); each refusal path; pre-existing spec refusal; help-verb output includes `brownfield`.

## 2. Control-socket + state plumbing

- [ ] 2.1 In `autocoder/src/control_socket/actions.rs`, add `BrownfieldAction { repo_url, capability_name, guidance: Option<String>, channel, thread_ts, request_id }`.
- [ ] 2.2 New module `autocoder/src/state/brownfield_request.rs` defining `BrownfieldRequestState` with atomic-rename writes (parallel to `ProposalRequestState`). Per-workspace path: `<workspace>/.state/brownfield_requests/<request_id>.json`.
- [ ] 2.3 Per-repo state extension: `pending_brownfield_requests: VecDeque<RequestId>` (parallel to `pending_proposal_requests`).
- [ ] 2.4 Tests: state file round-trip; queue enqueue/dequeue; concurrent-write safety.

## 3. Polling-loop brownfield handler

- [ ] 3.1 New module `autocoder/src/polling/brownfield.rs` exposing `process_pending_brownfield(repo_state, daemon_ctx) -> Result<()>`.
- [ ] 3.2 On each iteration, after the existing `process_pending_proposals` call AND before the standard change-processing pass, drain one brownfield request:
  - Load the request's state file.
  - Re-check `openspec/specs/<capability-name>/spec.md` does not exist; if it does, post a thread reply naming the late conflict, set state to `Aborted`, AND return.
  - Build the brownfield-draft prompt input (the embedded template + the operator's guidance + the workspace's `README.md` + the workspace's `docs/*.md` filenames + a code-symbol overview built via `cargo metadata` OR a ripgrep pass for top-level public items).
  - Invoke the executor in brownfield-draft mode (`WritePolicy::OpenSpecOnly`, sandbox: `Read`, `Glob`, `Grep`, `Bash` read-only).
- [ ] 3.3 On executor `Completed`:
  - Verify the change directory `openspec/changes/brownfield-<capability-name>/` exists AND contains `proposal.md`, `tasks.md`, AND `specs/<capability-name>/spec.md`.
  - Verify no source-file modifications outside `openspec/` (sandbox should prevent this, but verify via `git status --porcelain`); revert + WARN if any leak.
  - Create the spec branch (no fixes branch — brownfield is spec-only).
  - Open the PR with body templated from the proposal's "Why."
  - Set state to `Acted` with the PR URL.
- [ ] 3.4 On executor `Err` OR missing change-directory artifacts: post a thread reply naming the failure, set state to `Failed`, revert workspace.
- [ ] 3.5 Tests:
  - Mocked executor returns Completed with valid artifacts → spec PR created.
  - Mocked executor returns Completed with leaked source modifications → WARN logged + workspace reverted.
  - Mocked executor returns Completed with missing `specs/<cap>/spec.md` → state `Failed`, thread reply naming missing artifact.
  - Mocked executor returns Err → state `Failed`, thread reply naming error.

## 4. Brownfield-draft prompt template

- [ ] 4.1 Create `prompts/brownfield-draft.md`. Required content:
  - Role statement: "You are drafting a canonical OpenSpec capability spec for code that already exists. The capability is named <capability-name>. The operator may have provided guidance: <guidance>."
  - Process steps: (1) read the named capability's code surface; (2) read README + docs/*.md; (3) draft `openspec/changes/brownfield-<capability-name>/proposal.md`, `tasks.md`, AND `specs/<capability-name>/spec.md` with an `## ADDED Requirements` block.
  - Output rules:
    - Requirements describe observable behavior, not implementation detail. Scenarios are grounded in what the code actually does.
    - Use `SHALL` for normative statements; reserve commentary for the requirement body.
    - One coherent slice of behavior per requirement; do NOT lump unrelated behaviors.
    - If capability boundary is unclear, draft what is clear AND surface the ambiguity in proposal.md "Why."
  - Anti-noise rules: do NOT speculate about features that aren't in the code; do NOT propose new behavior; do NOT include implementation-level prose (file paths, function signatures) inside requirement bodies — those belong in the "Affected code" of the proposal if useful.
  - tasks.md guidance: review-oriented (validate spec against code), NOT implementation tasks.
- [ ] 4.2 Embed via `include_str!("../../prompts/brownfield-draft.md")` in the brownfield-polling module.
- [ ] 4.3 Operators override via `features.brownfield.prompt_path` (relative to workspace root).

## 5. Config integration

- [ ] 5.1 In `autocoder/src/config.rs`, extend the config schema with a top-level `features` block (if not present), AND within it `brownfield: { enabled: bool (default true), prompt_path: Option<String> (default None) }`.
- [ ] 5.2 The brownfield polling handler reads `features.brownfield.prompt_path`; if set AND the file exists, read it as the prompt template; otherwise fall back to the embedded default.
- [ ] 5.3 Tests: config with explicit override path parses; default-omitted parses; missing override file produces a clear WARN AND falls back to embedded.

## 6. Chatops notification surface

- [ ] 6.1 The ack message uses the standard `✓ Queued ...` shape (no per-verb emoji needed — brownfield reuses the existing top-level-ack pattern from `propose`).
- [ ] 6.2 Thread replies during processing: the standard polling-iteration thread-update mechanism applies. On Completed, post `✅ Brownfield draft PR opened: <pr_url>`.
- [ ] 6.3 On Failed, post `✗ Brownfield draft failed: <reason>` AND link to the daemon log.
- [ ] 6.4 Tests: notification messages match the documented shapes.

## 7. Docs

- [ ] 7.1 `docs/CHATOPS.md`: add a section for the `brownfield` verb under the chat-driven-workflow verbs (alongside `propose`, `audit`, `send it`). Include syntax, refusal cases, AND the lifecycle-thread behavior.
- [ ] 7.2 `docs/OPERATIONS.md`: add a paragraph under the "Onboarding existing projects" section (creating it if absent) describing brownfield-drafting as the first step, AND the relationship to `propose` for ongoing changes.
- [ ] 7.3 `docs/CONFIG.md`: document `features.brownfield.{enabled, prompt_path}` with defaults AND override semantics.
- [ ] 7.4 `config.example.yaml`: include the `features.brownfield` block commented out.

## 8. Spec deltas

- [ ] 8.1 `openspec/changes/a23-generate-brownfield/specs/chatops-manager/spec.md` ADDs the inbound-listener requirement AND the lifecycle-thread ack requirement.
- [ ] 8.2 `openspec/changes/a23-generate-brownfield/specs/orchestrator-cli/spec.md` ADDs the queueing requirement, the brownfield-draft executor mode requirement, AND the `features.brownfield` config-schema requirement.
- [ ] 8.3 `openspec/changes/a23-generate-brownfield/specs/project-documentation/spec.md` ADDs the docs requirement.

## 9. Verification

- [ ] 9.1 `cargo test` passes (new + existing).
- [ ] 9.2 `openspec validate a23-generate-brownfield --strict` passes.
- [ ] 9.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
- [ ] 9.4 Manual verification on a small unfamiliar repo: clone, configure as an autocoder workspace, invoke `@<bot> brownfield <repo> <pick-a-capability>`, review the resulting PR for spec quality. Iterate via `@<bot> revise` until the spec matches reality.
