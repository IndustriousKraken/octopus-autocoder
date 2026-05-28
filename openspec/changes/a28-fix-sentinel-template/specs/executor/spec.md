## ADDED Requirements

### Requirement: Sentinel emission instructions in the implementer prompt include a concrete worked example AND a self-check hint
Every outcome-sentinel format documented in `prompts/implementer.md` (currently the `SpecNeedsRevision` sentinel; future formats SHALL follow the same pattern) SHALL be presented with three structural elements:

1. **A substitution instruction** appearing IMMEDIATELY BEFORE the example, naming the rule that the example is a pattern AND that emitting it verbatim is a parse failure.
2. **A worked example with no angle-bracket placeholders** showing what a complete, parseable sentinel looks like. The example SHALL deserialize cleanly into the corresponding Rust type via `serde_json::from_str` AND SHALL contain realistic task ids, prose, AND reasoning that the agent can model.
3. **A self-check hint** appearing AFTER the example, instructing the agent to scan its emitted sentinel for `<...>` patterns inside string values before emitting AND describing the daemon's placeholder-detection diagnostic.

The implementer prompt SHALL NOT use angle-bracket placeholders (`<id-from-tasks-md>`, `<verbatim quote>`, etc.) inside string values in any sentinel example. Earlier versions of the prompt used this pattern AND triggered literal-emission failures; the lesson is preserved as a hard rule.

Operator-customizable override prompts (loaded via the uniform `PromptLoader` per `a24`'s spec) MAY use any structure the operator prefers — the canonical rule binds the bundled default only. Operators whose customized templates regress to placeholder-style examples will hit the same failure mode the bundled prompt previously hit; the placeholder-detection requirement in `orchestrator-cli` surfaces the diagnostic AND points the operator at the bundled default for reference.

#### Scenario: Bundled prompt's sentinel example is parseable
- **WHEN** an automated test deserializes the worked-example JSON from `prompts/implementer.md`'s sentinel section into `SpecNeedsRevisionDetail`
- **THEN** the deserialization succeeds without error
- **AND** every field's value is a concrete string (no angle-bracket markers, no template variables)

#### Scenario: Bundled prompt contains the three structural elements
- **WHEN** a maintainer reads `prompts/implementer.md`'s sentinel section
- **THEN** the section contains a substitution instruction paragraph IMMEDIATELY BEFORE the example
- **AND** the example itself contains no angle-bracket placeholders inside string values
- **AND** a self-check hint paragraph appears AFTER the example naming the daemon's placeholder-detection diagnostic

#### Scenario: Future sentinel formats follow the same pattern
- **WHEN** a future change introduces a new sentinel format in `prompts/implementer.md` (OR a new operator-aimed prompt template added by the daemon)
- **THEN** the new format's documentation in the prompt follows the substitution-instruction + worked-example + self-check-hint structure
- **AND** the new format's example deserializes cleanly into its corresponding Rust type
