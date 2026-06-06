# orchestrator-cli — delta for a011-assisted-install-and-doctor

## ADDED Requirements

### Requirement: Dependency preflight reports all dependencies; doctor subcommand
The daemon SHALL run a dependency preflight that checks every REQUIRED dependency AND every dependency implied by the active configuration, reporting the status of all of them together rather than failing on the first. Required dependencies — at minimum `openspec`, `git`, AND a usable platform sandbox mechanism — SHALL fail the preflight when missing. Configuration-implied dependencies — the agent-CLI binary for each configured strategy, a forge/scout CLI when those features are enabled, AND an embedding backend when RAG is enabled — SHALL be reported AND warned when missing, fatal only when their feature is active. The same check SHALL be available on demand as an `autocoder doctor` subcommand that prints the full report AND exits non-zero when a required dependency is missing. The sandbox-mechanism check SHALL verify the mechanism is USABLE — e.g. `bwrap` actually runs under the host's user-namespace policy — not merely present, complementing (not replacing) the spawn-time fail-closed sandbox gate. This extends the existing openspec-availability preflight to cover the full dependency set.

#### Scenario: All missing dependencies are reported together
- **WHEN** the preflight runs with more than one dependency missing
- **THEN** it reports all of them in one report
- **AND** does not stop at the first

#### Scenario: A missing required dependency fails the preflight
- **WHEN** a required dependency (`openspec`, `git`, or a usable sandbox mechanism) is missing
- **THEN** the preflight fails with a clear message naming it AND how to install it

#### Scenario: Configured-strategy binaries are checked, unconfigured ones are not
- **WHEN** a strategy is configured but its CLI binary is absent
- **THEN** the report marks it missing for that strategy
- **AND** binaries for strategies that are not configured are not required

#### Scenario: A present-but-unusable mechanism is reported unusable
- **WHEN** a sandbox-mechanism binary exists but cannot apply the sandbox (e.g. `bwrap` present but unprivileged user namespaces are disabled)
- **THEN** the check reports the mechanism as unusable, not satisfied

#### Scenario: doctor exits non-zero on a missing required dependency
- **WHEN** `autocoder doctor` runs with a required dependency missing
- **THEN** it prints the full report
- **AND** exits non-zero

### Requirement: Assisted dependency installation with per-step consent
The installer SHALL detect the host platform AND its package manager, AND offer to install the OS-package dependencies it can (e.g. bubblewrap, git, a forge/scout CLI), showing the exact command for each AND installing only on explicit per-step consent. For dependencies it cannot reliably auto-install — the agent CLIs (which have their own installers and interactive login) AND optional backends (e.g. Ollama) — it SHALL print the exact install and auth commands rather than attempting them. It SHALL NOT run a privileged install without first showing the command AND obtaining consent for that step.

#### Scenario: A missing OS package is offered with consent
- **WHEN** the installer finds a missing OS-package dependency AND a supported package manager
- **THEN** it shows the install command
- **AND** installs it only after the operator consents to that step

#### Scenario: Each install is its own consent step
- **WHEN** more than one OS-package dependency is missing
- **THEN** each is offered with its own consent step
- **AND** none is installed without its command shown

#### Scenario: Non-auto-installable dependencies get printed instructions
- **WHEN** a dependency cannot be auto-installed (an agent CLI or an optional backend)
- **THEN** the installer prints the exact install and auth commands
- **AND** does not attempt to run them

#### Scenario: No silent privileged install
- **WHEN** an install step requires elevated privilege
- **THEN** the command is shown AND consent obtained before it runs

### Requirement: Config path is discovered from the systemd unit
When a config path is not provided explicitly, `update.sh` AND the daemon CLI SHALL discover it from the installed systemd service unit — parsing the daemon's config argument out of the unit's `ExecStart`, matching the flag the daemon is actually launched with: the run command's `--config-dir <dir>` (from which the config file is `<dir>/config.yaml`), AND accepting a `--config <file>` form as well — so the operator does not retype a path that is already recorded. When no unit or no recorded config path is found, the existing default-path resolution applies. An explicitly provided config path SHALL always win and SHALL NOT consult the unit.

#### Scenario: Discovered from the unit
- **WHEN** no config path is provided AND the systemd unit's `ExecStart` launches the daemon with `--config-dir <dir>` (or `--config <file>`)
- **THEN** the resolver uses `<dir>/config.yaml` (or `<file>`)

#### Scenario: Falls back to default resolution
- **WHEN** no config path is provided AND no systemd unit records one
- **THEN** the existing default-path resolution applies

#### Scenario: An explicit path wins
- **WHEN** a config path is provided explicitly
- **THEN** it is used AND the systemd unit is not consulted
