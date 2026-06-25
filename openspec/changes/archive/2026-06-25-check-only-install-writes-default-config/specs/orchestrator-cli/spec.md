## ADDED Requirements

### Requirement: Check-only install writes its config to the default discovery path so verify needs no `--config`
The check-only install SHALL write its minimal config to the standard location `autocoder` auto-discovers — the same discovery the `run` subcommand uses when `--config` is omitted, which on a user install is `~/.config/autocoder/config.yaml`. A check-only config is an ordinary autocoder config (the same schema, carrying only the subset `verify` needs) AND SHALL NOT use a distinct filename. Consequently `autocoder verify <change-slug>` SHALL resolve this config via auto-discovery with NO `--config` flag. An explicit `--config <path>` SHALL continue to override discovery, so the installer's `--config` option AND CI invocations that pass an explicit path are unaffected. The installer's post-install summary SHALL present the next-step invocation that MATCHES where it wrote the config: the flagless `autocoder verify <change-slug>` when the config went to the default discovery path, OR `autocoder verify <change-slug> --config <path>` when an explicit `--config <path>` directed it elsewhere — so the suggested command always resolves the config the installer just wrote.

Writing the minimal config to the standard discovery path is safe because the check-only spec-authoring machine does not run the daemon, so there is no separate daemon `config.yaml` at that path for it to collide with; the existing "config already exists, leave it untouched" guard still prevents clobbering any pre-existing config.

#### Scenario: Check-only config lands at the auto-discovered path
- **WHEN** the check-only install completes on a user spec-authoring machine with no `--config` override
- **THEN** the minimal config is written to `~/.config/autocoder/config.yaml`
- **AND** `autocoder verify <change-slug>` run in a repository resolves that config via auto-discovery with no `--config` flag

#### Scenario: An explicit --config still overrides discovery
- **WHEN** an operator runs `autocoder verify <change-slug> --config <path>`
- **THEN** the config at `<path>` is used, overriding auto-discovery
- **AND** the installer's `--config <path>` option likewise writes the minimal config to that path
- **AND** the installer's post-install summary shows the matching `autocoder verify <change-slug> --config <path>` invocation

#### Scenario: The post-install summary shows the flagless invocation when the config went to the discovery path
- **WHEN** the check-only install finishes having written its config to the default discovery path (no `--config` override)
- **THEN** its printed next-step command is `autocoder verify <change-slug>` with no `--config` flag
