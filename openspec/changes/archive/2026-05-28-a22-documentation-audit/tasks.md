## 1. Registry + sandbox profile

- [x] 1.1 In `autocoder/src/audits/registry.rs` (or wherever audits are registered), add `documentation_audit` as a registered audit type with:
  - `requires_head_change: true`
  - `WritePolicy::None`
  - Sandbox: `Read`, `Glob`, `Grep`, `Bash` allowed; `Write` and `Edit` denied.
- [x] 1.2 New module `autocoder/src/audits/documentation_audit.rs` implementing the audit's `run(workspace, config) -> Result<AuditOutcome>`.
- [x] 1.3 Tests: registry returns the new type; sandbox profile matches the spec.

## 2. Embedded prompt template

- [x] 2.1 Create `prompts/documentation-audit.md`. Required content:
  - Role statement: "You are auditing the documentation of a repository against its implementation. Your job is to identify three classes of documentation defect..."
  - The three check categories with examples for each (coverage, stale_reference, organization).
  - Output format: a single JSON object `{ findings: [{ category, severity, anchor, body }] }`. No commentary outside the JSON.
  - Severity rules: `low` or `medium` only; do NOT emit `high` (documentation drift is rarely emergency-grade).
  - Anchor format: `<file>:<line>` for stale_reference findings; `<file>` for coverage AND organization findings.
  - Anti-noise rules: do NOT flag minor wording drift; do NOT flag implementation-detail comments that don't surface to operators; do NOT flag historical doc references to features that explicitly say "deprecated" or "removed."
  - Note about `extra.readme_max_lines` AND `extra.page_max_lines_without_toc` knobs the prompt should respect when assessing organization.
- [x] 2.2 Embed via `include_str!("../../prompts/documentation-audit.md")` in the audit module.
- [x] 2.3 Operators may override via `audits.settings.documentation_audit.prompt_path` (parallel to other audits).

## 3. Audit `run()` implementation

- [x] 3.1 Gather inputs:
  - All `<workspace>/openspec/specs/<cap>/spec.md` files.
  - All `<workspace>/README.md` AND `<workspace>/docs/*.md` files.
  - A code-symbol index: top-level public functions, structs, enums in `<workspace>/<source-tree>/` (Rust: `cargo metadata` OR a ripgrep pass for `pub fn`, `pub struct`, etc.; non-Rust: best-effort grep for top-level public items).
  - Optional: if `a21`'s RAG is enabled, the audit MAY use `query_canonical_specs` via the executor's MCP surface to fetch focused canonical context.
- [x] 3.2 Build the prompt: embedded template + the gathered inputs concatenated with `## File: <path>` headers.
- [x] 3.3 Invoke the executor in audit mode (the same surface other LLM-driven audits use).
- [x] 3.4 Parse the response JSON. On parse failure: log WARN naming the response excerpt, return `Err`. On parse success: convert to `AuditOutcome::Reported(findings)`.
- [x] 3.5 Reject findings with `severity: high` (the spec explicitly forbids); rewrite to `medium` AND log a WARN noting the demotion.
- [x] 3.6 Tests:
  - Mocked LLM returns `{"findings": []}` → audit returns `Reported(vec![])`.
  - Mocked LLM returns the three categories → each finding parses; severities pass through (or are demoted from `high`).
  - Mocked LLM returns malformed JSON → audit returns `Err` with the response excerpt.

## 4. Config integration

- [x] 4.1 In `autocoder/src/config.rs`, ensure `audits.defaults.documentation_audit` AND `audits.settings.documentation_audit` deserialize correctly. The audit-slug recognition test catches typos.
- [x] 4.2 Add `extra` knobs:
  - `readme_max_lines: usize` (default `200`).
  - `page_max_lines_without_toc: usize` (default `500`).
- [x] 4.3 The audit's `run()` reads these knobs from `settings.<slug>.extra` AND passes them to the prompt as part of the input.
- [x] 4.4 Tests: config with explicit knobs parses; defaults apply when omitted.

## 5. Chatops notification

- [x] 5.1 In the audit-notification module, add a case for `documentation_audit` using emoji `📚`. The top-line format:
  ```
  📚 documentation_audit on <repo-url>: <N> finding(s)
  ```
- [x] 5.2 The threaded body lists findings grouped by category (Coverage / Stale references / Organization). Each finding renders as `- <severity> at <anchor>: <body>`.
- [x] 5.3 Tests: notification text matches the format; threaded body groups by category correctly.

## 6. Install-wizard fast-path

- [x] 6.1 In the install wizard's `audits.defaults` setup, add `documentation_audit: monthly` to the recommended fast-path. The per-audit walk-through includes `documentation_audit` alongside the existing five.
- [x] 6.2 Add the CLI flag `--audit-documentation_audit <cadence>` to the non-interactive flow.
- [x] 6.3 Update `config.example.yaml` to include `documentation_audit` in the commented-out audits block.

## 7. Docs

- [x] 7.1 In `docs/OPERATIONS.md`'s `## Periodic audits` section, add a row to the audit table for `documentation_audit` describing the three check categories, the WritePolicy (`None`), AND the default cadence (`monthly` in the fast-path).
- [x] 7.2 Following the existing pattern for `architecture_consultative` AND `drift_audit`, add a paragraph after the table describing the audit's prompt structure, the three checks, AND the operator's workflow for acting on findings (`@<bot> send it` produces a docs-fix PR).
- [x] 7.3 In `docs/CONFIG.md`'s `audits.settings.<slug>.extra` discussion, add a paragraph mentioning the new audit's `readme_max_lines` AND `page_max_lines_without_toc` knobs.
- [x] 7.4 In `docs/CHATOPS.md`'s threaded-audit-notification documentation, add `📚` to the per-audit emoji list with a one-line description.

## 8. Spec deltas

- [x] 8.1 `openspec/changes/a22-documentation-audit/specs/orchestrator-cli/spec.md` ADDs `Documentation audit reports coverage, stale-reference, AND organization findings`.
- [x] 8.2 `openspec/changes/a22-documentation-audit/specs/chatops-manager/spec.md` ADDs `Documentation-audit chatops notification uses 📚 emoji`.
- [x] 8.3 `openspec/changes/a22-documentation-audit/specs/project-documentation/spec.md` ADDs `OPERATIONS.md AND CONFIG.md document the documentation_audit registered type`.

## 9. Verification

- [x] 9.1 `cargo test` passes (new + existing).
- [x] 9.2 `openspec validate a22-documentation-audit --strict` passes.
- [x] 9.3 `cargo clippy --all-targets --all-features -- -D warnings` produces no new warnings.
- [ ] 9.4 Manual verification on autocoder's own repo: enable the audit, trigger it via `@<bot> audit documentation coterie` (or this repo), inspect findings against known documentation drift. Expected catches based on recent observations: any remaining "feature shipped but undocumented" cases, any stale CLI/verb references in docs, organization issues we may not have addressed in the docs-reorg pass.
