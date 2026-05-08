# Apply Order

This file documents the order in which OpenSpec changes in `openspec/changes/` are intended to be applied. Each change builds on the prior baseline. Operators (and the orchestrator itself, when self-hosted) should process them in this order; an implementing agent should not begin a change until its predecessors have been merged.

## Order

1. **`orchestrator-architecture`** — establishes the four foundational capabilities (`orchestrator-cli`, `openspec-queue-engine`, `executor`, `git-workflow-manager`) and resolves the lock-recovery / dirty-workspace startup contract. The first task of this change is to promote `specs/<capability>/spec.md` files into the canonical `openspec/specs/` directory; everything downstream depends on that.

2. **`project-documentation`** — installs the global "Living Documentation" rule that subsequent implementing agents must obey. Apply early so all later changes already operate under the constraint.

3. **`orchestrator-foundation`** — full multi-repo daemon implementation of all four architecture capabilities, plus the new `workspace-manager` capability. Concrete `ClaudeCliExecutor` is the first executor backend. Real GitHub PR creation. Per-repo polling loops with iteration-level error tolerance and graceful SIGINT/SIGTERM shutdown. After this change, the orchestrator runs end-to-end against any number of configured repositories.

4. **`chatops-escalation`** — Slack integration for `ExecutorOutcome::AskUser`. Adds `chatops-manager` capability, modifies `orchestrator-cli` for the waiting-first iteration order plus same-repo strict-block on waiting changes, modifies `openspec-queue-engine` to filter waiting changes from pending and expose `list_waiting`. Implements AskUser detection in `ClaudeCliExecutor` and adds real `Executor::resume` semantics.

5. **`reviewer-integration`** — adds the `code-reviewer` capability (code-quality only, NOT spec compliance) and modifies `git-workflow-manager` to include the review report in the PR body and create draft PRs on Block verdicts. Opt-in via the `reviewer:` config block.

6. **`rewind-and-recovery`** — extends the basic rewind subcommand (implemented in `orchestrator-foundation` for single-repo only) to require a `--repo <selector>` argument when the config contains multiple repositories, with selector resolution by URL or short-name plus refined confirmation and partial-failure semantics.

## Future changes (not yet drafted)

- **`verifier`** — pre-commit spec-audit step using `/opsx:verify` plus configured shell commands; the `verifier` capability is added at this point, NOT earlier as a stub. See `verification_architecture.md` in this project's memory for the rationale.
- **`drift-audit`** — `orchestrator audit` subcommand walking every spec in `openspec/specs/` to produce a periodic drift report. Runs out-of-band with the daemon's normal polling.

## Notes

- The orchestrator does not enforce this order at runtime. The serial-queue invariant in each repository, plus the alphabetic sort of `openspec/changes/`, naturally enforces order *within* a single repository. This document is the human-facing source of truth for cross-change ordering.
- An implementing agent that picks up a downstream change before its predecessors have been merged should fail loudly because the architecture-level baselines won't be present in `openspec/specs/`. If you see a change failing for that reason, apply the missing predecessor first.
