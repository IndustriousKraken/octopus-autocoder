## Why

autocoder ships ten embedded prompt templates today and the override surface for them has accreted unevenly:

| Prompt                          | Override field                                  | Naming style |
|--------------------------------|--------------------------------------------------|--------------|
| `implementer.md`               | `executor.implementer_prompt_path`               | flat suffix  |
| `changelog-stylist.md`         | `executor.changelog_stylist_prompt_path`         | flat suffix  |
| `code-review-default.md`       | `reviewer.prompt_template_path`                  | flat, `_template_path` |
| `architecture-brightline.md`   | `audits.settings.architecture_brightline.prompt_path` | nested       |
| `architecture-consultative.md` | `audits.settings.architecture_consultative.prompt_path` | nested |
| `drift-audit.md`               | `audits.settings.drift_audit.prompt_path`        | nested       |
| `missing-tests-audit.md`       | `audits.settings.missing_tests_audit.prompt_path` | nested      |
| `security-bug-audit.md`        | `audits.settings.security_bug_audit.prompt_path` | nested       |
| `audit-triage.md`              | *none*                                            | —            |
| `chat-request-triage.md`       | *none*                                            | —            |
| `implementer-revision.md`      | *none*                                            | —            |

Three of these have no operator override at all. Each operator-customizable prompt also lacks a uniform load semantic — there is no single source of truth for "where does this prompt come from, in what precedence, and what happens when the file is missing."

The forward-looking changes already in the stack (`a23`'s `features.brownfield.prompt_path`, `a22`'s `audits.settings.documentation_audit.prompt_path`, the upcoming scout verb) compound the inconsistency unless the pattern is pinned down now.

This change codifies one pattern, fills the three override gaps, and produces a single in-docs registry of every prompt and its override.

## What Changes

**Uniform load semantics for every embedded prompt.** Each prompt SHALL be loaded through a single `PromptLoader` helper that:

1. Reads the embedded default template via `include_str!`.
2. Checks the prompt's per-repo override path (workspace-relative) — if set AND the file exists, use it.
3. Falls back to the legacy daemon-level override (where one exists) — if set AND the file exists, use it.
4. Falls back to the embedded default.
5. On a configured-but-missing override file, logs a one-shot WARN naming the missing path AND falls back.

The same helper is used by audit invocations, the executor, the reviewer, the changelog stylist, the brownfield handler, and the triage prompts. Each consumer SHALL call `PromptLoader::load(PromptId::X, &workspace_config)` instead of inlining `include_str!`.

**New per-workspace override fields for the three currently-unoverridable prompts**, using the nested naming convention going forward:

- `executor.audit_triage.prompt_path: Option<String>` for `prompts/audit-triage.md` (used during `send it` flows).
- `executor.chat_request_triage.prompt_path: Option<String>` for `prompts/chat-request-triage.md` (used during `propose` flows).
- `executor.implementer_revision.prompt_path: Option<String>` for `prompts/implementer-revision.md` (used during revision iterations).

All three default to `None`.

**Naming convention going forward.** New prompt overrides SHALL use the nested `<area>.<thing>.prompt_path` form (matching `audits.settings.<slug>.prompt_path` AND `features.brownfield.prompt_path`). Legacy flat fields (`executor.implementer_prompt_path`, `executor.changelog_stylist_prompt_path`, `reviewer.prompt_template_path`) remain accepted indefinitely for backward compatibility; the loader checks both.

**Prompt registry, documented centrally.** `docs/CONFIG.md` SHALL contain a "Prompt overrides" section with a single table listing every prompt: its logical id, its embedded path, its primary (per-workspace) override field, AND the legacy daemon-level field where one exists. The table is the operator's one-stop reference.

**No breaking changes.** Existing operator configs continue to work unchanged. The new per-workspace fields are additions; the legacy daemon-level fields keep their existing semantics.

## Impact

- **Affected specs:**
  - `executor` — ADDED requirement: `Prompt loader applies a uniform embedded → per-repo override → daemon-level override → embedded fallback precedence`. ADDED requirement: `executor.audit_triage.prompt_path, executor.chat_request_triage.prompt_path, AND executor.implementer_revision.prompt_path are per-workspace overrides for the three previously-unoverridable prompts`.
  - `orchestrator-cli` — ADDED requirement: `Triage prompts honor the per-workspace overrides`. (Triage invocations live in orchestrator-cli's polling-iteration; this requirement ties them to the new fields.)
  - `project-documentation` — ADDED requirement: `docs/CONFIG.md contains a Prompt overrides table covering every embedded prompt`.
- **Affected code:**
  - `autocoder/src/prompts/loader.rs` (new) — `PromptLoader::load(id, workspace_config) -> Result<String>` with `PromptId` enum.
  - Every consumer site that calls `include_str!("../../prompts/<X>.md")` directly — refactored to call the loader.
  - `autocoder/src/config.rs` — extend `PerRepoConfig.executor` with `audit_triage`, `chat_request_triage`, AND `implementer_revision` sub-blocks each containing an optional `prompt_path`.
  - `docs/CONFIG.md` — add the Prompt overrides section AND table.
- **Operator-visible behavior:**
  - Operators with existing configs see no change.
  - Operators can now override `audit-triage.md`, `chat-request-triage.md`, AND `implementer-revision.md` per workspace.
  - A configured-but-missing override path produces a single WARN log line per daemon-startup (not per load) AND uses the embedded default.
  - `docs/CONFIG.md` gains a single table operators can consult to find any prompt's override field.
- **Breaking:** no. All legacy fields remain accepted; new fields are additive.
- **Acceptance:** `cargo test` passes; `openspec validate a24-uniform-prompt-overrides --strict` passes. New tests:
  - Loader returns embedded when no override configured.
  - Loader returns per-workspace override when configured AND file exists.
  - Loader returns daemon-level override when per-workspace is unset AND legacy field IS set AND file exists.
  - Loader logs WARN once when configured path is missing, then falls back to embedded.
  - Each new override field (`executor.audit_triage.prompt_path` etc.) round-trips through the config parser.
  - A registry-completeness test asserts: for every `prompts/*.md` file, a `PromptId` enum variant exists.
