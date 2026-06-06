# executor — delta for a014-agent-toolchain-environment

## ADDED Requirements

### Requirement: Agentic subprocesses inherit the operator's activated toolchain environment, credential-filtered
The daemon SHALL capture the operator's login-shell environment — the activated `PATH` AND toolchain-activation variables (e.g. `PYENV_ROOT`, `RBENV_ROOT`, `NVM_DIR`, `CARGO_HOME`, `GOPATH`, `POETRY_*`) that shell initialization (`~/.bashrc` / `~/.profile`) sets up — AND provide it to every agentic subprocess through `agentic_run`, so toolchains activated by shell init (pyenv, rbenv, poetry, nvm) are usable, not merely present on disk. Capture SHALL be best-effort (dumping a login shell's environment) AND SHALL degrade gracefully: a partial or empty capture still proceeds with the base environment rather than failing the run.

The captured environment SHALL be **credential-filtered**: it propagates `PATH` and toolchain-activation variables but SHALL NOT propagate variables matching credential patterns — names containing `TOKEN`, `SECRET`, `KEY`, or `PASSWORD`, or known provider prefixes such as `AWS_` / `ANTHROPIC_` — so secrets the operator's shell exports never reach the model, including provider API keys (which as an env value would also bill the wrapped CLI off its subscription, per the key-flow requirement). The exclusion set SHALL ship with defaults AND be operator-editable. Where a captured variable conflicts with a variable the run itself sets (sandbox or strategy), the run's value SHALL take precedence.

#### Scenario: A shell-activated toolchain is runnable in the subprocess
- **WHEN** a toolchain is activated only by the operator's shell init (e.g. `pyenv` / `poetry` via `~/.bashrc`) AND the captured environment is provided to the agentic subprocess
- **THEN** the toolchain's commands resolve and run in the subprocess (the managed `python` / `poetry`), not the bare system fallback

#### Scenario: Credential variables are not propagated
- **WHEN** the operator's login-shell environment exports a credential-bearing variable (e.g. `FOO_TOKEN` or `ANTHROPIC_API_KEY`)
- **THEN** that variable is excluded from the environment provided to the agentic subprocess

#### Scenario: Run-set variables take precedence
- **WHEN** a captured variable conflicts with one the sandbox or strategy sets for the run
- **THEN** the run's value is used, not the captured one

#### Scenario: Partial capture degrades gracefully
- **WHEN** the login-shell environment capture fails or returns only a partial environment
- **THEN** the agentic run still proceeds with the base environment, without crashing or aborting
