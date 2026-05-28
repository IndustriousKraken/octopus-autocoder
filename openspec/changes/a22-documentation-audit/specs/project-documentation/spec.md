## ADDED Requirements

### Requirement: OPERATIONS.md, CONFIG.md, and CHATOPS.md document the documentation_audit registered type
`docs/OPERATIONS.md` SHALL include `documentation_audit` in the audit table in the `## Periodic audits` section AND a follow-up paragraph describing the three check categories AND the `@<bot> send it` workflow for acting on findings. `docs/CONFIG.md` SHALL document the audit's `extra` knobs (`readme_max_lines`, `page_max_lines_without_toc`). `docs/CHATOPS.md` SHALL note the `📚` emoji convention in its per-audit-emoji listing.

#### Scenario: OPERATIONS.md table includes the new audit
- **WHEN** an operator reads `docs/OPERATIONS.md`'s `## Periodic audits` section
- **THEN** the audit table contains a `documentation_audit` row with the audit's WritePolicy (`None`), whether it's LLM-driven (yes), default cadence (`monthly` in the fast-path), AND a one-line description naming the three check categories
- **AND** a follow-up paragraph elaborates on the three categories (coverage, stale-reference, organization), AND describes the operator workflow via `@<bot> send it` to produce a docs-fix PR

#### Scenario: CONFIG.md documents the `extra` knobs
- **WHEN** an operator reads `docs/CONFIG.md`'s `audits.settings.<slug>.extra` discussion
- **THEN** a paragraph describes the documentation_audit's `extra` knobs: `readme_max_lines: usize` (default `200`) AND `page_max_lines_without_toc: usize` (default `500`)
- **AND** notes that these are thresholds the LLM applies when emitting organization findings; operators in larger projects raise them, operators in smaller projects keep defaults

#### Scenario: CHATOPS.md emoji listing includes 📚
- **WHEN** an operator reads `docs/CHATOPS.md`'s per-audit-emoji discussion
- **THEN** a `📚 documentation_audit on <repo-url>: <N> finding(s)` example appears alongside the existing `📐`, `🧭`, `📋`, AND other per-audit emojis
- **AND** the note clarifies that documentation_audit findings ship via the threaded-notification path (top-line in channel, body in thread) on lengths exceeding the existing threshold
