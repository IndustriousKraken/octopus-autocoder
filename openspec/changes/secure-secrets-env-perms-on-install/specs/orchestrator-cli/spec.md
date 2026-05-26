## ADDED Requirements

### Requirement: Install wizard creates secrets file atomically with restrictive mode

The `autocoder install` subcommand SHALL create the `secrets.env` file
with mode `0o600` in the same syscall that creates the file. The
secrets file SHALL NEVER exist on disk with a mode wider than `0o600`,
even transiently between creation and a subsequent `chmod`. The
implementation MAY use `OpenOptions::mode(0o600).create_new(true)`
(or equivalent), `OpenOptions::mode(0o600).truncate(true)` over an
existing file, or any other mechanism that atomically associates the
creation event with mode `0o600`.

The `config.yaml` file SHALL be created with its target mode in the
same syscall — `0o600` in dev mode, `0o640` in server mode — using
the same approach. The post-write `chmod` calls MAY remain as
defense-in-depth but MUST NOT be the sole mechanism gating
permissions.

#### Scenario: Fresh install creates secrets.env with mode 0600 atomically

- **WHEN** `autocoder install` runs against a host with no existing
  `secrets.env` AND the wizard collects at least one secret (a
  GitHub PAT, a ChatOps bot token, or a reviewer API key)
- **THEN** the resulting file at `<config_dir>/secrets.env` has mode
  exactly `0o600` (owner read+write, no group, no other) as observed
  by `stat`
- **AND** at no point during the install does any process other than
  the install process and the eventual owner have permission to read
  the file's bytes

#### Scenario: Re-install over existing wider-perm secrets.env tightens before write

- **WHEN** `autocoder install --upgrade` runs against a host whose
  existing `secrets.env` has mode `0o644` (perhaps from a prior
  install that pre-dated this requirement) AND the wizard collects
  new secrets
- **THEN** the install path tightens the existing file to `0o600`
  BEFORE writing any new secret bytes into it (e.g. via
  `chmod`-then-truncate-then-write, or by removing the old file
  first and creating a new one with `OpenOptions::mode(0o600)`)
- **AND** the resulting file has mode `0o600` after the install
  completes
