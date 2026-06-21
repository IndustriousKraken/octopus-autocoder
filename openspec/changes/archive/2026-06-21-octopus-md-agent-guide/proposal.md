# OCTOPUS.md — an in-repo agent guide to the workflow protocols

## Why

A repository under management carries `openspec/` and `issues/`, but anyone
opening that repo has no in-repo explanation of how those protocols work.
Autocoder's own agents learn the rules from injected prompts; a non-autocoder
coding agent or speccing agent run directly on the repo — and human teammates —
get nothing, and routinely break protocol (editing canon, archiving early,
malforming a delta or an issue). A committed `OCTOPUS.md` at the repo root closes
the gap: it states the issues format, the OpenSpec change format, the
canon/archive ownership rules, and the gate model in one place any reader can
find. `AGENTS.md` (the conventional agent-guide spot) points to it, and every
default prompt directs the agent to read it when present. The file reaches the
base branch the only way the daemon can put content there — a dedicated pull
request — and is opt-out per repo for operators who do not want metafiles.

## What Changes

- ADD a `project-documentation` standard: managed repos carry a committed
  `OCTOPUS.md` agent guide (plus an `AGENTS.md` reference to it) that states the
  issues protocol, the OpenSpec change protocol, the canon/archive ownership
  rules, and the gate model. The file serves non-autocoder agents and humans;
  for autocoder's own agents the same rules are enforced by the gates and
  sandbox, which OCTOPUS.md does not replace.
- ADD an `orchestrator-cli` requirement: the daemon provisions `OCTOPUS.md` and
  the `AGENTS.md` reference through the established push + pull-request flow — it
  writes them ON THE AGENT BRANCH (after the per-iteration base sync recreates
  the agent branch), commits them, and rides the same push + PR-creation path any
  change uses, honoring the per-repo `auto_submit_pr` (a PR when enabled; a pushed
  branch with no PR / `BranchPushedNoPr` when false). It is NOT written at init as
  an untracked file (dirty-recovery would wipe it) and NOT committed to the base
  branch outside a PR (canon forbids it; base-sync would discard it). Provisioning
  is gated by a per-repo feature flag (`features.octopus_guide`, default ENABLED)
  and is idempotent: no write and no PR when the guide is already current, and no
  write and no PR at all when the flag is disabled.
- MODIFY the existing `Default prompts are language- and project-neutral`
  requirement to also direct each default prompt under `prompts/` to read
  `OCTOPUS.md` when present (graceful no-op when absent).

## Impact

- Affected specs: `project-documentation` (ADD the OCTOPUS.md standard; MODIFY
  the default-prompts requirement), `orchestrator-cli` (ADD the dedicated-PR
  provisioning requirement: agent-branch write + commit + push + PR honoring
  `auto_submit_pr`, the `features.octopus_guide` flag, idempotency, and the
  `AGENTS.md` reference).
- Affected code: `autocoder/src/config.rs` (`FeaturesConfig` +
  `features.octopus_guide` flag), the daemon's pass/provisioning path
  (`autocoder/src/polling_loop/commits.rs` base sync + agent-branch recreate,
  `autocoder/src/polling_loop/pass.rs` push + PR open, `autocoder/src/polling_loop/pr_open.rs`
  `auto_submit_pr` handling), the OCTOPUS.md content source and AGENTS.md
  refresh helper, `autocoder/src/git.rs` (`add_all`/`commit`), the default prompt
  files under `prompts/` and `autocoder/src/prompts/loader.rs`, `docs/OPERATIONS.md`.
- Prose duplication between OCTOPUS.md and the prompts is accepted: each prompt
  already restates the formats it needs; this change adds a single committed
  reference doc, not a refactor to unify those sources.
