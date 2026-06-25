# Check-only install writes its config to the default discovery path (no `--config` needed)

## Why

The check-only installer (`install-verify.sh`) writes its minimal config to
`~/.config/autocoder/verify.yaml`. But `autocoder`'s config auto-discovery looks
for `config.yaml` (`/etc/autocoder/config.yaml`, then
`~/.config/autocoder/config.yaml`) — NOT `verify.yaml`. So a freshly check-only-
installed machine cannot run `autocoder verify <slug>`: it errors with "no config
path provided … no config file at the default locations", and the operator must
pass `--config ~/.config/autocoder/verify.yaml` on every invocation.

There is no reason for the distinct filename. A check-only config is an ordinary
autocoder config — the same schema, just a minimal subset (the three gate model
blocks and the corpus location). The spec-authoring machine never runs the daemon,
so there is no `config.yaml` for it to collide with; writing the minimal config to
the standard discovery path is safe and makes `verify` work flagless, the way a
drop-in installer should.

## What Changes

- The check-only install SHALL write its minimal config to the standard
  auto-discovered location — on a user install, `~/.config/autocoder/config.yaml`
  (the same path `run` discovers) — instead of `verify.yaml`. As a result,
  `autocoder verify <change-slug>` resolves the config with NO `--config` flag.
- An explicit `--config <path>` continues to override discovery (unchanged), so
  the installer's `--config` option and CI uses that pass a path still work.
- The installer's post-install summary presents the flagless
  `autocoder verify <change-slug>` invocation. Documentation that shows
  `verify … --config ~/.config/autocoder/verify.yaml` is updated to the flagless
  form.

## Impact

- Affected specs: `orchestrator-cli` (one ADDED requirement; it adds the
  config-location / no-`--config` guarantee without restating the existing
  `verify` subcommand or check-only-install requirements).
- Affected code: `install-verify.sh` (the default `CONFIG_PATH` and the
  post-install summary), `docs/CLI.md` (the `verify` example). No change to the
  `verify` subcommand's own config-resolution logic — it already falls back to the
  same discovery `run` uses when `--config` is omitted.
- Independent change; touches no requirement another in-flight change modifies.
