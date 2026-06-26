# Tasks

## 1. Flip the config default

- [x] 1.1 Change `default_issues_enabled()` in `config.rs` to return `true`. Update the config unit tests that assert the default-off representation (`features.issues.enabled` defaults true; an absent `features.issues` block deserializes to an enabled lane).

## 2. Invert the install wizard

- [x] 2.1 Flip the interactive issues-lane gate to default-YES (keep on). Reword the prompt to offer disabling for operators who track corrections in an external tracker (Jira, Linear, and similar), while still stating that enabling makes unit selection `issues > changes > audits` AND that autonomous GitHub-issue triage is a separate flag (`features.scout.include_issues`) this does not turn on.
- [x] 2.2 Wizard output: keeping the default writes no `features.issues` entry; opting out writes `features.issues.enabled: false`.
- [x] 2.3 Flip the non-interactive `--issues-lane` default to `enabled`. `--issues-lane disabled` writes `features.issues.enabled: false`; `--issues-lane enabled` writes `features.issues.enabled: true`. Update the install unit tests that asserted the default-off / opt-in behavior.

## 3. Changelog

- [x] 3.1 Add a changelog entry calling out the default change: existing installs whose config omits `features.issues` gain the active issues lane on upgrade, with the one-line opt-out (`features.issues.enabled: false`).

## 4. Tests

- [x] 4.1 Config: an absent `features.issues` block resolves to an enabled lane; an explicit `enabled: false` resolves to a disabled lane (round-trip).
- [x] 4.2 Install wizard: the default interactive path and the non-interactive no-flag path both produce an on lane (no `features.issues` entry); the opt-out paths produce `features.issues.enabled: false`. Assert behavior (the resolved lane state), not prompt wording.
