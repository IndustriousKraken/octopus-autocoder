# Tasks

## 1. Installer writes to the discovery path

- [ ] 1.1 In `install-verify.sh`, change the default `CONFIG_PATH` from `${HOME}/.config/autocoder/verify.yaml` to `${HOME}/.config/autocoder/config.yaml` (the standard user discovery location). The `--config <path>` option still overrides the default.
- [ ] 1.2 Preserve the existing "config already exists; leaving it untouched" guard against the new path, so a check-only install never clobbers a pre-existing `config.yaml`.
- [ ] 1.3 Update the post-install summary so the printed next-step command is `autocoder verify <change-slug>` with NO `--config` flag (still print the resolved config path for reference).

## 2. Docs

- [ ] 2.1 Update `docs/CLI.md`: replace the `autocoder verify add-widget-endpoint --config ~/.config/autocoder/verify.yaml` example with the flagless `autocoder verify add-widget-endpoint` form, noting that a check-only install's config is auto-discovered. Adjust any other `verify.yaml` references (`docs/INSTALL.md`) to match.

## 3. Tests

- [ ] 3.1 Assert (script-level or integration) that the check-only installer, run with no `--config`, writes its minimal config to `~/.config/autocoder/config.yaml`, and that `autocoder verify <slug>` resolves a config from that discovery path with no `--config` flag.
- [ ] 3.2 Assert that an explicit `--config <path>` still overrides discovery (the installer writes to the given path; `verify --config <path>` uses it).
