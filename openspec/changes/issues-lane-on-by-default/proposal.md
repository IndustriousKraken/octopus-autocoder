# Issues lane on by default

## Why

The issues lane is one of the two fundamental work paths — the home for
behavior-preserving corrections (bug fixes, refactors) that carry no spec delta,
and now the default destination for the implementation defects the bug/gap audits
find. Shipping it off by default makes a fundamental path dormant: a default
install cannot route a correction to the issues lane at all, and audit findings
that should be issues have nowhere to go. An operator who tracks corrections in an
external system (Jira, Linear, and similar) is the exception, and should opt out,
rather than every operator having to opt in to a core path.

Flipping the default is safe. The `features.issues` flag activates the issues
walker and the `issues > changes > audits` unit-selection precedence. It does NOT
turn on autonomous triage of public GitHub issues — that is separately gated by
`features.scout.include_issues` and remains opt-in. So default-on does not
introduce any autonomous public-facing behavior; it makes the daemon work
committed and audit-written issue units.

## What Changes

- The `features.issues.enabled` field defaults to `true`. An operator disables the lane by
  setting `features.issues.enabled: false`.
- The install wizard's issues-lane gate inverts from opt-in to opt-out: the lane
  is on by default, and the prompt offers to disable it for operators who track
  corrections in an external tracker. The prompt continues to state what enabling
  the lane does (the `issues > changes > audits` precedence) and clarifies that
  autonomous GitHub-issue triage stays behind `features.scout.include_issues`.
- The non-interactive `--issues-lane` flag's default flips to `enabled`.

## Migration impact

This is a deliberate change to the default. An existing install whose config has
no explicit `features.issues` entry will have the issues lane active after
upgrade. That is the intended outcome (the lane becomes fundamental), but it is a
real on-upgrade behavior change and warrants a changelog callout so operators who
do not want the lane know to set `features.issues.enabled: false`.

## Impact

- Affected specs: `orchestrator-cli` (`Issues lane for corrections`,
  `Install wizard configures the issues lane`).
- Affected code: `default_issues_enabled()` in `config.rs`; the install wizard's
  issues-lane prompt and the `--issues-lane` flag default.
- Composes with `a01-auditors-choose-lane`, which keys off the flag's runtime
  value, not its default.
