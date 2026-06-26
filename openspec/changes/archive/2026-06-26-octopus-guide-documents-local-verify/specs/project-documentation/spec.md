## ADDED Requirements

### Requirement: OCTOPUS.md documents the local verify pre-check
Beyond the protocols listed in the `Managed repos carry a committed OCTOPUS.md agent guide` requirement, OCTOPUS.md SHALL also tell readers that, before pushing a change, they MAY run the local verify pre-check — `autocoder verify <change-slug>` — to learn whether the change will pass the server gates before the server evaluates it. OCTOPUS.md SHALL state that this runs the same `[in]` / `[canon]` / `[rules]` gates the daemon runs pre-executor, against the change in the local working tree; that it is READ-ONLY (it does not run the executor, edit specs, or write markers); AND that it is a feedback ACCELERATOR, NOT a replacement for the server gates, which remain the fail-closed enforcement (they run against fresher canon at implement time AND cover every contributor).

OCTOPUS.md SHALL note that the `verify` subcommand ships in the autocoder binary AND is usable without running the daemon via the check-only install, AND that a gate reporting it "could not run" (fail-closed: model unconfigured, transport error, or no resolvable rule corpus) is an ENVIRONMENT/CONFIG condition, not a spec defect — so the fix is the config, not the change.

This requirement adds to the guide's content only. It does NOT restate the gate model (per the `The gate model` protocol of the agent-guide requirement) NOR alter the provisioning mechanism (per the daemon's OCTOPUS.md provisioning requirement) — it points readers at the local surface of those same gates.

#### Scenario: OCTOPUS.md describes the local verify pre-check
- **WHEN** an agent or human reads OCTOPUS.md before pushing a change
- **THEN** it learns it MAY run `autocoder verify <change-slug>` to run the `[in]` / `[canon]` / `[rules]` gates locally, in the working tree, before the server evaluates the change
- **AND** it is told that verify is read-only AND is a feedback accelerator, not a replacement for the server gates

#### Scenario: OCTOPUS.md frames verify as available via the check-only install
- **WHEN** a reader without a running daemon consults OCTOPUS.md's verify section
- **THEN** it states that the `verify` subcommand ships in the autocoder binary AND is usable without the daemon via the check-only install

#### Scenario: OCTOPUS.md distinguishes a real finding from a gate that could not run
- **WHEN** OCTOPUS.md's verify section is read
- **THEN** it states that a gate reporting it "could not run" (fail-closed) is an environment/config condition, not a spec defect — the fix is the config, not the change
