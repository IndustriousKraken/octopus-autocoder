# orchestrator-cli — delta for a014-agent-toolchain-environment

## ADDED Requirements

### Requirement: doctor verifies toolchains are runnable in the agent environment
Beyond checking presence, the dependency `doctor` / preflight SHALL verify that the expected toolchains are RUNNABLE in the agent's actual environment — the captured, credential-filtered environment applied under the sandbox policy — by invoking each (e.g. `<tool> --version`) AND reporting any that are present on disk but NOT runnable (for example, a version manager whose shim/init did not activate, so the managed interpreter does not resolve), with an actionable hint. The expected set SHALL default to a common toolchain list AND be operator-configurable. A present-but-not-runnable toolchain SHALL be reported so the activation gap surfaces at startup rather than mid-run.

#### Scenario: A present-but-not-runnable toolchain is reported
- **WHEN** a toolchain is present on disk but not runnable in the agent environment (e.g. `pyenv` installed but not initialized, so `python` does not resolve to the managed interpreter)
- **THEN** `doctor` reports it as present-but-not-runnable with a hint to activate it (e.g. add its init to the unit's environment)

#### Scenario: A runnable toolchain passes
- **WHEN** an expected toolchain runs successfully in the agent environment (`<tool> --version` succeeds)
- **THEN** `doctor` reports it as available

#### Scenario: The expected set is configurable
- **WHEN** the operator configures the expected-toolchain list
- **THEN** `doctor` checks exactly that set, defaulting to the common list when unset
