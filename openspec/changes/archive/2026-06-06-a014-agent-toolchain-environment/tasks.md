# Implementation tasks

## 1. Capture the operator's login-shell environment

- [x] 1.1 At daemon startup, dump the operator's login-shell environment (e.g. `bash -lc 'env'`; attempt an interactive login where feasible so `.bashrc`-guarded init like `pyenv init` runs). Parse `PATH` and toolchain-activation variables.
- [x] 1.2 Degrade gracefully: a failed or partial capture leaves the base environment in place; never abort a run over a capture miss.

## 2. Credential filter

- [x] 2.1 Exclude variables matching credential patterns (names containing `TOKEN`, `SECRET`, `KEY`, `PASSWORD`, or provider prefixes such as `AWS_` / `ANTHROPIC_`) from the captured environment, so shell-exported secrets — and provider API keys (also a subscription-billing hazard, per a003) — never reach the subprocess.
- [x] 2.2 Ship the exclusion set as defaults; make it operator-editable (mirroring a013's mask-list).

## 3. Inject into `agentic_run`

- [x] 3.1 Compose the captured + filtered environment into every agentic subprocess's environment, alongside the sandbox / strategy env. On conflict, the run-set value (sandbox / strategy) takes precedence over the captured one.

## 4. `doctor` runnability check

- [x] 4.1 Extend the `doctor` (a011) to run each expected toolchain (`<tool> --version`) in the agent's actual environment (captured env under the sandbox policy) and report any present-on-disk-but-not-runnable, with an activation hint.
- [x] 4.2 Default the expected-toolchain set to a common list; make it operator-configurable.

## 5. Tests

- [x] 5.1 A toolchain activated only by shell init (`pyenv` / `poetry`) is runnable in the agent subprocess once the captured env is applied.
- [x] 5.2 A credential-pattern variable (`FOO_TOKEN`, `ANTHROPIC_API_KEY`) is excluded from the captured environment.
- [x] 5.3 A run-set variable wins over a conflicting captured variable.
- [x] 5.4 A failed/partial capture proceeds with the base environment (no abort).
- [x] 5.5 `doctor` reports a present-but-not-runnable toolchain with a hint; a runnable one passes; the expected set is configurable.

## 6. Acceptance gate

- [x] 6.1 `cargo test` passes for the autocoder crate.
- [x] 6.2 `cargo clippy --all-targets -- -D warnings` is clean.
- [x] 6.3 `openspec validate a014-agent-toolchain-environment --strict` passes.
