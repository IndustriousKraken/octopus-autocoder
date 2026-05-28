## ADDED Requirements

### Requirement: Documentation audit reports coverage, stale-reference, and organization findings
autocoder SHALL register a `documentation_audit` audit type in the periodic-audit framework. The audit is LLM-driven, declares `WritePolicy::None`, `requires_head_change = true`, AND a sandbox profile allowing `Read`, `Glob`, `Grep`, AND `Bash` (read-only). It produces `AuditOutcome::Reported(findings)` covering three categories of documentation defect:

1. **Coverage** — code or canonical-spec features that user-facing docs (`README.md`, `docs/*.md`) don't mention. Heuristic: any canonical-spec requirement whose body mentions operator-visible artifacts (`@<bot>` verbs, config keys, CLI flags, file paths the operator interacts with) is in scope. Pure-internal capabilities are NOT flagged.
2. **Stale references** — docs references to code symbols (function names in code blocks, CLI verbs, config fields, file paths under `src/`) that don't exist in the current code or canonical specs. Catches dead references from removed features.
3. **Organization** — qualitative structural findings: README exceeding `extra.readme_max_lines` lines (default `200`), docs pages exceeding `extra.page_max_lines_without_toc` (default `500`) without a TOC, important user-visible features buried below setup/admin material on their page, two docs pages covering the same topic without cross-linking, capabilities surfaced only in CHANGELOG but never in operator docs.

The audit's findings SHALL be tagged with `severity` of `low` OR `medium` ONLY — the audit deliberately does NOT emit `high` (documentation drift is rarely emergency-grade; promotion would crowd out genuinely-urgent audit signals from other types). An `anchor` field names `<file>:<line>` for stale-reference findings AND `<file>` (no line) for coverage AND organization findings.

The audit's prompt template `prompts/documentation-audit.md` ships embedded via `include_str!` AND is overridable via `audits.settings.documentation_audit.prompt_path`. Two `extra` knobs apply: `readme_max_lines` (default `200`) AND `page_max_lines_without_toc` (default `500`). The prompt receives these knobs as part of its input AND respects them when emitting organization findings.

The audit does NOT produce LLM-generated documentation proposals (unlike `missing_tests_audit` / `security_bug_audit`). Findings ship as `Reported` outcomes; operators run `@<bot> send it` in the audit's threaded notification to trigger a triage executor run that produces a docs-fix PR (NOT a spec PR). The PR participates in the standard `@<bot> revise <text>` revision loop.

When `a21`'s canonical-spec RAG is enabled in the same workspace, the audit's prompt MAY use the `query_canonical_specs` MCP tool to fetch focused canonical context. The audit functions correctly without RAG too; the RAG integration is an opportunistic enhancement, not a requirement.

#### Scenario: Audit detects implementation-without-documentation
- **WHEN** the canonical spec contains a requirement whose body mentions an operator-visible feature (e.g. `@<bot> propose` verb)
- **AND** none of `README.md` or `docs/*.md` mentions `propose`
- **THEN** the audit emits a finding with `category: coverage`, `severity: medium`, `anchor: <docs-or-spec-file-where-the-feature-is-defined>`, AND a body explaining the missing documentation

#### Scenario: Audit detects documentation-without-implementation
- **WHEN** `docs/CONFIG.md` references a config field `executor.foo_bar_quux` in a code block
- **AND** no Rust source file under `<workspace>/<source-tree>/` defines a field named `foo_bar_quux` in any struct
- **THEN** the audit emits a finding with `category: stale_reference`, `severity: medium`, `anchor: docs/CONFIG.md:<line>`, AND a body naming the missing referent

#### Scenario: Audit detects organization issues
- **WHEN** `docs/CHATOPS.md` is 600 lines long AND has no top-of-file TOC
- **AND** the page documents user-driving workflows (`propose`, `send it`) AND administrative recovery verbs (`clear-perma-stuck`)
- **AND** the user-driving content appears below the admin material
- **THEN** the audit MAY emit findings with `category: organization`, `severity: low` or `medium`, naming each separately (missing TOC; burial of user-driving content)

#### Scenario: Audit deliberately does not emit `high` severity
- **WHEN** the LLM's response contains a finding marked `"severity": "high"`
- **THEN** the audit demotes it to `"medium"` AND logs a WARN naming the demotion
- **AND** the operator-visible finding lists severity `medium`

#### Scenario: Audit honors `requires_head_change = true`
- **WHEN** the audit's `last_run_sha` equals the current base-branch HEAD AND the cadence has elapsed
- **THEN** the framework skips the audit (per the existing framework requirement)
- **AND** the next iteration after a HEAD change re-evaluates

#### Scenario: Pure-internal capability is NOT flagged for coverage
- **WHEN** a capability's canonical spec exists BUT every requirement body covers pure-internal mechanics (no operator-visible artifacts)
- **THEN** the audit does NOT emit a coverage finding for that capability
- **AND** the heuristic recognizes "internal" via the absence of `@<bot>` verbs, config keys, CLI flags, AND operator-facing file paths in the requirement bodies

#### Scenario: `extra` knobs apply to organization thresholds
- **WHEN** `audits.settings.documentation_audit.extra.readme_max_lines: 400`
- **AND** `README.md` is 300 lines
- **THEN** the audit does NOT emit a "README too long" finding (the threshold is operator-raised)
- **WHEN** the same config AND `README.md` grows to 500 lines
- **THEN** the audit emits the organization finding

#### Scenario: Audit works without `a21`'s RAG
- **WHEN** `canonical_rag` is disabled (no block OR `enabled: false`)
- **AND** `documentation_audit` runs
- **THEN** the audit completes successfully without invoking `query_canonical_specs`
- **AND** findings are emitted based on the prompt's direct access to canonical specs (read via the sandbox's `Read` tool)

#### Scenario: Audit uses RAG when available
- **WHEN** `canonical_rag` is enabled AND a documentation_audit run starts
- **THEN** the audit's executor invocation has access to `query_canonical_specs` via MCP
- **AND** the prompt MAY direct the LLM to use the tool for canonical-context retrieval
- **AND** the implementation detail (whether the LLM uses the tool) is left to the prompt's design — both with-RAG AND without-RAG produce valid output

#### Scenario: Findings can be acted on via `send it`
- **WHEN** the audit posts a threaded notification with findings AND the operator replies `@<bot> send it` in that thread
- **THEN** the existing `audit-reply-acts` mechanism triggers a triage executor run
- **AND** the triage produces a doc-fix PR (changes to `README.md` / `docs/*.md` files)
- **AND** the triage does NOT produce a spec PR (documentation is not OpenSpec material)
- **AND** the doc-fix PR participates in the standard `@<bot> revise <text>` revision loop
