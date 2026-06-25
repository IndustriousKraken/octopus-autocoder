## ADDED Requirements

### Requirement: Audit thread body includes each finding's full body, not just its title line

The rendered audit thread body SHALL include, per finding, the existing
`severity glyph + subject + anchor` title line FOLLOWED BY that finding's full
`body` field when the body is non-empty — so the operator reading the audit
notification sees what the spec requires, what the code does, and why the gap
matters (for `drift_audit`, the divergence paragraph), not merely
`[capability] title (file:line)`. A finding whose `body` is empty SHALL still
render its one-line title, with no stray blank-body artifact. Findings SHALL be
visually separated so each finding's title and body read as one unit.

This requirement constrains only the CONTENT of the thread body produced for the
audit-findings notification (the "full findings body" referenced by the existing
"Audit findings post via the threaded-notification path when long enough to
benefit" requirement). It does NOT change the threading threshold, the per-type
top-line, OR the truncation cap. The rendered body — now richer — remains subject
to the existing "Thread body truncates at 35,000 characters with a pointer to the
daemon log" requirement: when the body would exceed 35,000 characters it is
truncated to that cap and ends with the existing pointer-to-daemon-log tail.

#### Scenario: A drift finding's divergence body appears in the thread body

- **WHEN** a `drift_audit` produces a finding whose `body` is a divergence paragraph (what the spec requires, what the code does, why it matters)
- **THEN** the rendered thread body contains the finding's `severity glyph + subject + anchor` title line AND the full divergence paragraph from `body`
- **AND** the body text is present verbatim, not reduced to the one-line title

#### Scenario: A body-less finding renders its title with no blank-body artifact

- **WHEN** an audit produces a finding whose `body` field is empty
- **THEN** the rendered thread body contains that finding's one-line title
- **AND** no stray blank line or empty-body placeholder is emitted for it

#### Scenario: The richer body is still capped at 35,000 characters

- **WHEN** the rendered body — including the finding bodies — would exceed 35,000 characters
- **THEN** it is truncated to 35,000 characters AND ends with the existing pointer-to-daemon-log tail, per the existing "Thread body truncates at 35,000 characters with a pointer to the daemon log" requirement
- **AND** the 35,000 cap value AND the tail text are unchanged by this requirement
