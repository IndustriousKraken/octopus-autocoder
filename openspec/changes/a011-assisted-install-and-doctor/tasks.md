# Implementation tasks

## 1. Dependency preflight + `doctor`

- [ ] 1.1 Add a dependency-preflight module that checks, in one pass: `openspec`, `git`, a usable platform sandbox mechanism, the agent-CLI binary for each configured strategy, a forge/scout CLI when those features are enabled, and an embedding backend when RAG is enabled. Collect ALL results before reporting (do not stop at the first failure).
- [ ] 1.2 Make the sandbox-mechanism check verify USABILITY, not mere presence: on Linux, that `bwrap` actually runs (probe under the host userns policy) or `systemd-run` service mode is usable; on macOS, that `sandbox-exec` is present.
- [ ] 1.3 Wire the preflight into startup, extending the existing openspec-availability preflight to the full set; required-missing fails startup with an actionable message; feature-implied-missing warns unless the feature is active.
- [ ] 1.4 Add an `autocoder doctor` subcommand that runs the same check, prints the full report, and exits non-zero when a required dependency is missing.

## 2. Assisted installer with per-step consent

- [ ] 2.1 Detect the platform and package manager (apt / dnf / pacman / zypper / brew).
- [ ] 2.2 For each missing OS-package dependency (bubblewrap, git, forge/scout CLI), show the exact install command and install only on explicit per-step consent. Never run a privileged install without showing the command and getting consent for that step.
- [ ] 2.3 For dependencies that cannot be auto-installed (the agent CLIs and optional backends like Ollama), print the exact install + auth commands instead of running them.

## 3. Config-path discovery from the systemd unit

- [ ] 3.1 In `update.sh` and the daemon CLI's config resolution: when no config path is given, parse the installed systemd unit's `ExecStart` for the `--config` argument and use it.
- [ ] 3.2 Fall back to the existing default-path resolution when no unit or recorded path is found; an explicitly provided path always wins and skips the unit lookup.

## 4. Docs

- [ ] 4.1 Document `autocoder doctor` and the assisted installer in INSTALL.md / DEPLOYMENT.md, including the sandbox-mechanism prerequisite (bubblewrap on an unprivileged Linux daemon; `sandbox-exec` on macOS).

## 5. Tests

- [ ] 5.1 The preflight reports multiple missing dependencies together (not just the first).
- [ ] 5.2 A missing required dependency fails the preflight with an actionable message; `autocoder doctor` exits non-zero.
- [ ] 5.3 An absent configured-strategy CLI is reported missing for that strategy; an unconfigured strategy's CLI is not required.
- [ ] 5.4 A present-but-unusable `bwrap` (userns disabled) is reported unusable, not satisfied.
- [ ] 5.5 The installer offers each missing OS package with its own consent step and shows the command; a non-auto-installable dependency gets printed instructions instead of an install attempt.
- [ ] 5.6 The config path is read from a systemd `ExecStart --config`; absent a unit it falls back to defaults; an explicit path wins and skips the unit lookup.

## 6. Acceptance gate

- [ ] 6.1 `cargo test` passes for the autocoder crate.
- [ ] 6.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [ ] 6.3 `openspec validate a011-assisted-install-and-doctor --strict` passes.
