# OCTOPUS.md documents the local verify pre-check

## Why

`OCTOPUS.md` already tells any agent or human working in a managed repo how to
develop here — the issues protocol, the OpenSpec change protocol, canon/archive
ownership, and the gate model. What it does NOT mention is that an author can run
those gates LOCALLY before pushing: `autocoder verify <change-slug>` runs the same
`[in]` / `[canon]` / `[rules]` checks the daemon runs pre-executor, against the
change in the working tree, so the author learns whether the change will pass
before the server evaluates it.

That pre-check is the single highest-leverage habit for anyone authoring specs
here — it catches self-contradictions, canon conflicts, and rule violations (and,
in practice, even subtle internal contradictions in a freshly written delta)
before a round-trip through the daemon. Because `OCTOPUS.md` is the seeded,
tool-agnostic guide every agent and human reads — and it self-refreshes through
autocoder's provisioning PR flow — it is the right home for this, rather than an
agent-specific skill or slash command that only reaches one toolchain.

## What Changes

- `OCTOPUS.md` SHALL gain a short section telling readers they MAY run
  `autocoder verify <change-slug>` before pushing a change, stating that it runs
  the same `[in]` / `[canon]` / `[rules]` gates locally, that it is read-only, and
  that it is a feedback ACCELERATOR — not a replacement for the server gates, which
  remain the fail-closed enforcement.
- The section SHALL note that `verify` ships in the autocoder binary and is usable
  without the daemon via the check-only install, AND that a gate reporting it
  "could not run" (fail-closed) is an environment/config condition, not a spec
  defect.

## Impact

- Affected specs: `project-documentation` — one ADDED requirement extending the
  guide's content. It does not restate the gate model or change the provisioning
  mechanism; it points readers at the local surface of the same gates.
- Affected code: the `OCTOPUS_MD` content source in `autocoder/src/octopus_guide.rs`
  (the single deterministic source the provisioner writes AND stale-compares). The
  managed repo's committed `OCTOPUS.md` — including this repo's own root copy — is
  re-provisioned from that source through the existing push + PR flow; no manual
  edit of any committed `OCTOPUS.md` is required.
- Independent change; touches no requirement another in-flight change modifies, and
  no provisioning behavior changes.
