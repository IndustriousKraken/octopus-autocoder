# Documentation

This directory holds the long-form documentation for autocoder. The main [README](../README.md) covers what it is, how to install it, and where to find everything else; the files here are the reference material an operator consults occasionally.

## Index

- [INSTALL.md](INSTALL.md) — Manual install from source. The `autocoder install` wizard handles most cases; use this for contributor setups, air-gapped builds, or installs that need to inspect the build itself.
- [CONFIG.md](CONFIG.md) — Full `config.yaml` schema. Every field, every default. Includes the multi-token routing for operators running across more than one GitHub owner.
- [CHATOPS.md](CHATOPS.md) — ChatOps escalation, operator commands, the Socket-Mode inbound listener setup, and the experimental non-Slack backends.
- [CODE-REVIEW.md](CODE-REVIEW.md) — The optional AI code-reviewer's scope, verdict semantics, prompt template, and PR composition rules.
- [OPERATIONS.md](OPERATIONS.md) — Operating notes: workspace paths, queue order, busy markers, perma-stuck recovery, spec-needs-revision recovery, self-heal, audits, rebuilding canonical specs, runtime config reload, dirty-workspace auto-recovery.
- [DEPLOYMENT.md](DEPLOYMENT.md) — Recommended binary deploy with systemd: user setup, SSH keys, the unit file, env-var layout, upgrade workflow, applying config changes without a restart.
- [SECURITY.md](SECURITY.md) — AI security & guardrails: credential scoping, branch protection, the self-modifying-AI risk, workspace isolation, secrets in config (inline vs env-var), dedicated user, fork-and-PR workflow, executor tool sandbox.
- [CLI.md](CLI.md) — `run`, `reload`, `rewind` subcommand reference.
- [TROUBLESHOOTING.md](TROUBLESHOOTING.md) — Diagnosing common failure modes, including rebuild failures.
- [foundation-smoke-test.md](foundation-smoke-test.md) — Manual smoke-test walkthrough for first-time deploys.
- [test-reliability.md](test-reliability.md) — Notes on the test suite's reliability story.
