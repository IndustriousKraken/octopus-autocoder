# Design

## D1 — Default-on rationale

The issues lane is one of the two primary lanes, not an optional feature. With
`a01`, the bug/gap audits route implementation defects to it by default. A core
path that is off by default is dormant for every operator who did not find and
flip the flag. Inverting the default makes the lane available everywhere and
relocates the decision to the genuine exception: an operator who already tracks
corrections in an external system.

## D2 — Opt-out install wizard

The wizard's single issues-lane gate inverts: the default is keep-on, and the
operator can choose to disable. The prompt body still states the effect that
matters — enabling the lane changes per-iteration unit selection to
`issues > changes > audits` — and adds the clarification that autonomous triage
of open GitHub issues is a separate flag (`features.scout.include_issues`) that
default-on does NOT turn on. This keeps the operator informed in both directions:
what the lane does, and what it deliberately does not do.

The escape hatch is named: an operator who uses Jira, Linear, or a similar
external tracker opts out here. This is also the natural seam for later
ticket-system integration, if anyone asks for it.

The non-interactive `--issues-lane <enabled|disabled>` flag keeps its shape; only
its default flips to `enabled`.

## D3 — Config representation

`default_issues_enabled()` returns `true`. The default-on state is represented by
the absence of a `features.issues` entry (matching the schema default), so:

- keep the default (lane on) → the wizard writes no `features.issues` entry;
- opt out (lane off) → the wizard writes `features.issues.enabled: false`;
- explicit non-interactive enable → `features.issues.enabled: true`.

## D4 — Scope boundary

This change flips only the lane walker's default. It does NOT touch
`features.scout.include_issues` (autonomous public-issue triage stays opt-in), and
it does NOT change the issues lane's mechanics (walker, precedence, promotion,
archival) — only whether the lane is active when unconfigured. `a01` is
unaffected; it reads the flag's resolved value.

## D5 — Migration

Flipping a default is a behavior change for already-deployed installs whose config
omits `features.issues`: they gain the active lane on upgrade. This is intended.
The changelog SHALL call it out, with the one-line opt-out
(`features.issues.enabled: false`) for operators who do not want it.
