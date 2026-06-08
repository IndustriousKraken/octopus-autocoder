## Why

`a013` makes the host toolchains **visible** to the executor (home exposed), but visible is not active. systemd does not source `~/.bashrc`, so the shell init that *activates* `pyenv` / `rbenv` / `poetry` / `nvm` — the shims on `PATH`, `PYENV_ROOT` / `NVM_DIR`, `eval "$(pyenv init -)"`, the virtualenv — never runs. The agent finds the files but `python` resolves to the wrong interpreter (or none), and it either burns tokens reverse-engineering the activation or silently builds against the system fallback.

This change captures the operator's **activated login-shell environment** and injects it into every agentic subprocess so the toolchains are usable, and extends the `doctor` to verify toolchains are actually **runnable** in the agent's environment — not merely present.

## What Changes

**Login-shell environment capture.** The daemon captures the operator's login-shell environment (the activated `PATH` plus toolchain-activation variables like `PYENV_ROOT`, `RBENV_ROOT`, `NVM_DIR`, `CARGO_HOME`, `GOPATH`, `POETRY_*`) and provides it to every agentic subprocess through `agentic_run`. Capture is best-effort (dump a login shell's environment) and degrades gracefully — a partial or empty capture still runs against the base environment rather than failing.

**Credential-filtered.** The captured environment propagates `PATH` and toolchain variables but **excludes** variables matching credential patterns (`TOKEN`, `SECRET`, `KEY`, `PASSWORD`, provider prefixes like `AWS_` / `ANTHROPIC_`), so secrets the operator's shell exports never reach the model — including provider API keys, which as an env value would also bill the wrapped CLI off its subscription (per `a003`). The exclusion set ships with defaults and is operator-editable, mirroring `a013`'s mask-list. Where a captured variable conflicts with one the run itself sets (sandbox / strategy), the run's value wins.

**`doctor` verifies runnability.** Beyond presence, the `doctor` (from `a011`) runs each expected toolchain (`<tool> --version`) in the agent's actual environment and reports any that are present-on-disk but not runnable — e.g. a version manager whose init did not activate — with an actionable hint. The activation gap becomes a boot-time line, not a mid-run flail.

## Impact

- **Affected specs:** `executor` — ADD `Agentic subprocesses inherit the operator's activated toolchain environment, credential-filtered`. `orchestrator-cli` — ADD `doctor verifies toolchains are runnable in the agent environment`.
- **Affected code:** a startup env-capture (run the operator's login shell, dump and parse `PATH` + toolchain vars, apply the credential filter); `agentic_run` injects the captured + filtered environment into the subprocess, composing with the sandbox / strategy env with run-set values taking precedence; the `doctor` runs `<tool> --version` for the expected set in the agent environment and reports non-runnable toolchains.
- **Operator-visible behavior:** `pyenv` / `rbenv` / `poetry` / `nvm` toolchains the operator uses interactively work for the agent with no per-tool configuration; shell-exported secrets are not handed to the model; `doctor` flags present-but-not-activated toolchains at startup.
- **Dependencies:** stacks on `a013` (the sandbox exposes the toolchains) AND `a011` (the `doctor`). Complements `a003` (the credential filter keeps provider keys out of the subprocess env).
- **Acceptance:** `cargo test` passes; `cargo clippy --all-targets -- -D warnings` is clean; `openspec validate a014-agent-toolchain-environment --strict` passes. Tests: a shell-init-activated toolchain (`pyenv`/`poetry`) is runnable in the agent subprocess; a credential-pattern variable (`FOO_TOKEN`, `ANTHROPIC_API_KEY`) is excluded from the captured env; a run-set variable wins over a captured one on conflict; a partial/empty capture degrades without aborting; `doctor` reports a present-but-not-runnable toolchain with a hint.
