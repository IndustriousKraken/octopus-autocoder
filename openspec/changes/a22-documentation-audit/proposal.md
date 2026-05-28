## Why

The existing audit suite covers code (`architecture_brightline`, `architecture_consultative`, `security_bug_audit`), tests (`missing_tests_audit`), AND spec/code alignment (`drift_audit`). Documentation is the gap. We've already hit operator-visible symptoms of documentation drift on autocoder's own repo this week:

- Shipped features that nobody is told about (the `propose` and `send it` verbs were live for days before any documentation surfaced them — they were buried near the bottom of `docs/CHATOPS.md` for most of the rollout).
- The README listed `drift_audit` as "aspirational; not in any active change" long after the audit had shipped.
- Documentation references to renamed configuration fields (the `recreate_fork_on_reinit` typo case caught by the config-coverage test) AND to removed code paths slip through with no automated check.
- README and CHATOPS.md both grew to where major features were buried under setup/admin material. Organization drift accumulates over many small PRs and surfaces only when an operator complains.

`drift_audit` handles spec-vs-code alignment AND spec-vs-spec contradiction. A documentation_audit would handle three orthogonal documentation concerns the other audits don't catch:

1. **Implementation-without-documentation**: code or canonical-spec features that user-facing docs don't mention. Catches the "we shipped it but never told anyone" pattern.
2. **Documentation-without-implementation**: docs references to functions, files, config fields, CLI verbs, OR chatops verbs that don't exist in code (or in canonical specs). Catches stale docs from removed features.
3. **Organization findings**: docs file structure that hurts operator self-service — over-long READMEs, missing TOCs where length warrants them, important features buried below setup/admin material, sections that should cross-link AND don't.

The audit is LLM-driven (the three checks are not mechanically computable; they need semantic understanding of what's documented AND why), `requires_head_change = true` (only re-runs when docs OR code change), AND ships findings via the existing `📋`/`📚`-prefixed chatops surface. Acting on findings goes through the standard `send it` triage flow, which produces doc-fix PRs.

## What Changes

**New `documentation_audit` registered audit type.** Same registry pattern as the existing five audits. Declared `WritePolicy::None`, `requires_head_change = true`. Sandbox allows `Read`, `Glob`, `Grep`, `Bash` (the same read-only sandbox used by `drift_audit` AND `architecture_consultative`).

**The audit's three checks (in one LLM call):**

1. **Coverage** — for each capability with a canonical `openspec/specs/<cap>/spec.md` file AND for each user-facing concern in the codebase (CLI subcommands, chatops verbs, config-block fields, behavioral defaults), the audit checks whether user-facing docs (`README.md`, `docs/*.md`) mention it. Capabilities with NO user-visible behavior (e.g. pure-internal modules) are not flagged. Heuristic: any requirement whose body mentions operator-visible artifacts (`@<bot>`, config keys, CLI flags, file paths the operator interacts with) is in scope for coverage.

2. **Stale references** — for each docs reference to a code symbol (function name in a code block, CLI verb, config field, file path under `src/`), the audit verifies the referent exists. Missing → finding. Heuristic anchors: `\`<symbol>\`` AND `path/to/file.rs:LN` AND `@<bot> <verb>` patterns the audit can grep against the current codebase + specs.

3. **Organization** — qualitative structure findings. Examples (the prompt lists these AND the LLM may surface others):
   - README.md exceeds ~200 lines of body (excluding code blocks) without explicit organizational discipline.
   - A docs page exceeds ~500 lines without a top-of-file TOC or table.
   - A user-facing feature page (e.g. CHATOPS.md) buries the major operator-driven workflows below setup/admin material.
   - A capability with major user surface area is mentioned only in CHANGELOG, never in operator docs.
   - Two docs pages cover the same topic without cross-linking.

**Prompt template at `prompts/documentation-audit.md`.** Embedded via `include_str!`; overridable via `audits.settings.documentation_audit.prompt_path`. The prompt encodes the three-check structure AND emits findings in the standard audit-finding JSON shape per the existing audit framework.

**Output shape — `AuditOutcome::Reported(findings)`.** Each finding has `category` (one of `coverage`, `stale_reference`, `organization`), `severity` (`low` or `medium` only; the audit deliberately does not emit `high` — documentation drift is rarely "drop everything and fix"), `anchor` (file:line OR file path), AND `body` (one paragraph). The chatops surface posts the standard threaded-notification format with a `📚 documentation_audit on <repo>: <N> finding(s)` top-line.

**Configuration:**

```yaml
audits:
  defaults:
    documentation_audit: monthly  # disabled | daily | every-N-days | weekly | monthly | quarterly
  settings:
    documentation_audit:
      prompt_path: null
      notify_on_clean: false
      extra:
        readme_max_lines: 200             # threshold for "README too long" finding
        page_max_lines_without_toc: 500   # threshold for "missing TOC" finding
```

Default cadence in the install wizard's recommended fast-path: `monthly`. Documentation drifts on a slower timescale than code; weekly is overkill. Operators with rapid-iteration repos may bump it to weekly via the per-repo override.

**Acting on findings.** Operators run `@<bot> send it` in the audit's threaded notification (per the existing `audit-reply-acts` mechanism). The triage LLM reads the findings AND produces a fixes PR (doc edits) — NOT a spec PR, because docs are not OpenSpec material. The PR participates in the standard PR-comment revision loop.

**The audit does NOT produce LLM-generated docs proposals like `missing_tests_audit` does.** Documentation changes have semantic risk (LLMs are prone to introducing inaccuracies in docs) AND benefit from operator review at PR time, not from automated commits. The `Reported` outcome surface AND `send it` triage path are the right shape.

**Interaction with `a21`'s RAG.** When `a21`'s canonical-spec RAG is enabled, the audit's prompt MAY use it (via the `query_canonical_specs` MCP tool, since the audit runs in the same executor surface) to fetch relevant canonical context efficiently. The audit prompt mentions this opportunity but doesn't require RAG — operators without a21 enabled see the audit work fine, just with potentially less canonical context per check.

## Impact

- **Affected specs:**
  - `orchestrator-cli` — one ADDED requirement: `Documentation audit reports coverage, stale-reference, AND organization findings`.
  - `chatops-manager` — one ADDED requirement: `Documentation-audit chatops notification uses 📚 emoji`.
  - `project-documentation` — one ADDED requirement: `OPERATIONS.md AND CONFIG.md document the documentation_audit registered type`.
- **Affected code:**
  - `autocoder/src/audits/documentation_audit.rs` (new) — implements the audit's `run()`. Mostly: load the prompt, gather the relevant inputs (canonical specs + docs/*.md + a code-symbol index built via `cargo metadata` OR ripgrep), invoke the executor in audit mode, parse the findings.
  - `autocoder/src/audits/registry.rs` (or wherever audits are registered) — register the new type alongside the existing five.
  - `prompts/documentation-audit.md` (new) — embedded prompt template per the proposal.
  - `autocoder/src/config.rs` — extend `AuditsConfig.settings` to accept the new audit's `extra` knobs (`readme_max_lines`, `page_max_lines_without_toc`).
  - `autocoder/src/chatops/audit_notification.rs` — add the `📚` emoji case for `documentation_audit` (parallel to the existing per-audit emoji mapping for brightline / drift / etc.).
  - The install-wizard fast-path AND CLI flags gain `documentation_audit` to the recommended-cadence list (default `monthly`).
  - `docs/OPERATIONS.md` — extend the `## Periodic audits` section's audit table with the new type.
  - `docs/CONFIG.md` — extend the `audits.defaults` AND `audits.settings.<slug>.extra` discussion with the new audit's knobs.
- **Operator-visible behavior:**
  - With `audits.defaults.documentation_audit: monthly` (or any cadence): the audit fires on schedule + HEAD-change, posting a `📚 documentation_audit on <repo>: <N> finding(s)` top-line with threaded findings body.
  - Operators act via `@<bot> send it` to produce a doc-fix PR.
  - Findings surface the three categories with clear anchors so operators can navigate to the issue directly.
  - The audit's per-run log captures the full prompt + findings for review.
- **Breaking:** no. Disabled by default unless the operator opts in. The install wizard's fast-path recommends `monthly` for new installs (or as an explicit add-on for existing installs that re-run the wizard).
- **Acceptance:** `cargo test` passes; `openspec validate a22-documentation-audit --strict` passes. New tests:
  - The audit registers as a recognized audit type.
  - Sandbox profile is `Read, Glob, Grep, Bash` (read-only).
  - Audit output parses correctly for each of the three finding categories.
  - Chatops top-line includes the `📚` emoji.
  - `extra.readme_max_lines` AND `extra.page_max_lines_without_toc` config knobs apply.
