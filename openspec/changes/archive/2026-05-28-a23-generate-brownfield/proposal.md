## Why

autocoder's spec-driven loop assumes `openspec/specs/<capability>/spec.md` already exists for every capability the operator wants to evolve. Onboarding an existing codebase to autocoder therefore requires writing canonical specs by hand for whatever already exists — historically an ad-hoc LLM-assisted exercise without a fixed shape.

A chatops verb that drafts an initial canonical spec for one named capability gives this exercise consistency:

- **For the operator's own projects** retrofitting spec-driven development, brownfield-drafting one capability at a time is the natural granularity. The operator names what to spec, the LLM reads the code, and the resulting PR is reviewed and iterated like any other spec PR.
- **For OSS-contribution workspaces** (specs maintained in a sibling repo separate from the upstream project), brownfield is the bootstrap step that produces the initial canonical-spec set the rest of autocoder's machinery reads from.

`propose` exists for "implement something new"; brownfield is its inverse — "document something that already exists." The two verbs cover the full lifecycle of bringing an existing project under spec-driven development AND then evolving it.

## What Changes

**New `brownfield` chatops verb.** Syntax:

```
@<bot> brownfield <repo-substring> <capability-name> [optional guidance text]
```

The repo-substring follows the established case-insensitive substring-match rule (per `propose` AND `audit`). The capability-name is the slug the new spec will live under at `openspec/specs/<capability-name>/spec.md`; it SHALL match `^[a-z][a-z0-9-]*$`. The optional guidance text is everything after the capability-name, trimmed AND capped at 10,000 characters; it is passed verbatim to the executor prompt as operator-supplied guidance (focus areas, scope notes, naming preferences).

The dispatcher SHALL emit a `BrownfieldRequest` control-socket action with `{ repo_url, capability_name, guidance: Option<String>, channel, thread_ts }`, write a `BrownfieldRequestState` file with `status: Pending`, AND post a top-level ack message whose `ts` becomes the request's lifecycle thread.

**Refusal cases at parse time:**

- Missing capability name → reply with usage hint, no state file written.
- Capability name fails the slug pattern → reply naming the constraint, no state file written.
- Repo substring ambiguous → reply with the existing `match_repo`-style candidate list, no state file written.
- `openspec/specs/<capability-name>/spec.md` ALREADY exists in the resolved repo's workspace → reply pointing operator to `@<bot> propose ...` for changes to an existing capability, no state file written. (Detected at dispatch time via the workspace's HEAD; if the file appears between dispatch AND the polling iteration, the iteration aborts with a thread reply.)

**New executor mode: brownfield-draft.** The polling iteration picks up `pending_brownfield_requests` AND invokes the executor with a brownfield-draft prompt instead of the standard change-proposal prompt. The executor's sandbox is `WritePolicy::OpenSpecOnly` — the LLM can only write within `openspec/` (no source-file modifications).

The brownfield-draft prompt directs the LLM to:

1. Read the codebase to identify the named capability's surface area (modules, public functions, configuration knobs, user-visible behaviors).
2. Read `README.md` AND `docs/*.md` for any existing user-facing description of the capability.
3. Draft a change proposal at `openspec/changes/brownfield-<capability-name>/`:
   - `proposal.md` — "Why" explains that this captures existing behavior under canonical specs (no behavioral change); "What Changes" enumerates the requirements being added; "Impact" lists the single affected capability AND notes "no code changes."
   - `tasks.md` — review-oriented tasks: validate each requirement against the named code modules, confirm scenarios match observable behavior, run any existing test suite for the capability.
   - `specs/<capability-name>/spec.md` — an `## ADDED Requirements` block containing one requirement per coherent slice of the capability's behavior, with `#### Scenario:` blocks grounded in what the code actually does.
4. If the capability's boundary is unclear (the LLM cannot reconcile the operator-supplied name with one cohesive slice of the codebase), surface the ambiguity in the proposal's "Why" section AND draft a best-effort spec covering what the LLM did identify. Operators iterate via the standard `@<bot> revise <text>` loop.

**Prompt template AND override.** Default prompt lives embedded at `prompts/brownfield-draft.md` via `include_str!`. Per-workspace override: `features.brownfield.prompt_path: <path-or-null>` (relative to workspace root). The brownfield verb participates in the broader per-workspace-prompt pattern; the override knob's location is provisional AND MAY be relocated when the general prompt-override schema is formalized.

**Configuration:**

```yaml
features:
  brownfield:
    enabled: true             # operator can disable per-workspace
    prompt_path: null         # operator may point at a custom prompt
```

The default is `enabled: true` everywhere. The verb is harmless when disabled — the dispatcher refuses with `✗ brownfield: disabled in this workspace's config (features.brownfield.enabled=false).`

**PR shape.** Brownfield runs produce a spec-only PR (no fixes PR, since brownfield never modifies code). The PR participates in the standard `@<bot> revise <text>` revision loop. After merge, the standard `openspec archive` mechanism moves `openspec/changes/brownfield-<capability-name>/` to `openspec/changes/archive/<date>-brownfield-<capability-name>/` AND lands `openspec/specs/<capability-name>/spec.md`.

**Interaction with subsequent work.** Once the brownfield PR merges, `openspec/specs/<capability-name>/spec.md` is canonical. Any future change to that capability follows the standard `propose` → review → merge → archive pipeline; brownfield's job is done.

## Impact

- **Affected specs:**
  - `chatops-manager` — ADDED requirement: `Inbound listener recognizes the brownfield verb AND submits a BrownfieldAction`. ADDED requirement: `brownfield-verb ack uses the lifecycle-thread pattern`.
  - `orchestrator-cli` — ADDED requirement: `brownfield chatops verb queues a brownfield-draft executor request`. ADDED requirement: `Brownfield-draft executor mode produces a spec-only change PR`. ADDED requirement: `features.brownfield config schema`.
  - `project-documentation` — ADDED requirement: `docs/CHATOPS.md, docs/OPERATIONS.md, AND docs/CONFIG.md document the brownfield verb`.
- **Affected code:**
  - `autocoder/src/chatops/listener.rs` (or the inbound-verb-dispatch module) — add `brownfield` to the recognized-verb list, parse `<capability-name>` + optional guidance, emit `BrownfieldAction`.
  - `autocoder/src/control_socket/actions.rs` — add `BrownfieldAction` enum variant.
  - `autocoder/src/state/brownfield_request.rs` (new) — `BrownfieldRequestState` parallel to `ProposalRequestState`.
  - `autocoder/src/polling/brownfield.rs` (new) — picks up `pending_brownfield_requests`, invokes executor in brownfield-draft mode, handles the spec-only PR creation path.
  - `autocoder/src/config.rs` — add `features.brownfield.{enabled, prompt_path}`.
  - `prompts/brownfield-draft.md` (new) — embedded default prompt template.
  - `docs/CHATOPS.md`, `docs/OPERATIONS.md`, `docs/CONFIG.md` — verb + config documentation.
- **Operator-visible behavior:**
  - `@<bot> help` lists `brownfield` alongside the existing verbs.
  - `@<bot> brownfield <repo> <capability-name> [guidance]` produces a spec-only PR adding `openspec/specs/<capability-name>/spec.md`.
  - Standard `@<bot> revise <text>` iteration applies on the resulting PR.
- **Breaking:** no. New verb, opt-in per-workspace (`features.brownfield.enabled` defaults to `true` but the verb does nothing unless invoked).
- **Acceptance:** `cargo test` passes; `openspec validate a23-generate-brownfield --strict` passes. New tests:
  - Listener parses `@<bot> brownfield <repo> <cap-slug>` with AND without guidance text.
  - Slug-pattern rejection produces the expected reply.
  - Pre-existing `openspec/specs/<cap>/spec.md` causes dispatcher-level refusal.
  - Brownfield-draft executor mode invocation passes guidance through to the prompt.
  - `features.brownfield.enabled: false` produces the disabled-verb reply.
