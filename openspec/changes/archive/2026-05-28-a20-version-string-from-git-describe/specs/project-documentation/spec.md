## ADDED Requirements

### Requirement: DEPLOYMENT.md and CHATOPS.md explain the version-string format and the source-vs-binary distinction
`docs/DEPLOYMENT.md` SHALL include a "Version-string format" section explaining how the daemon resolves its version string at build time, what operators see in different build contexts (clean tag, dev commit past tag, dirty working tree, source tarball without `.git/`), AND the Cargo.toml-bump convention. `docs/CHATOPS.md` SHALL update the `🆙` startup-notification example to show both the clean-tag form AND the development-build form.

#### Scenario: DEPLOYMENT.md describes every build context
- **WHEN** an operator reads `docs/DEPLOYMENT.md`'s "Version-string format" section
- **THEN** the section names the four build contexts (clean tag, dev commits past tag, dirty working tree, tarball without `.git/`) AND the corresponding version-string output for each
- **AND** the section explains that Cargo.toml's `version =` field is the "base version operators manually bump at semver-meaningful releases" — NOT bumped per-commit
- **AND** the section notes that binary-release installs (via `update.sh`) always see clean `vX.Y.Z` strings because the release workflow builds at tagged commits

#### Scenario: CHATOPS.md shows both notification forms
- **WHEN** an operator reads `docs/CHATOPS.md`'s `🆙` startup-notification documentation
- **THEN** the example shows both forms:
  - `🆙 autocoder v1.1.1 started — 8 repository(ies) configured` (clean tag)
  - `🆙 autocoder v1.1.1-23-g4abc123 started — 8 repository(ies) configured` (dev commits past tag)
- **AND** a one-liner explains when each form appears
