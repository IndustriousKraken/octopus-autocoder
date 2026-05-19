## 1. Drift-audit timeout branch

- [x] 1.1 `run_returns_err_on_subprocess_timeout` (in
  `autocoder/src/audits/drift.rs` tests module) — write a fake CLI
  script whose body is `#!/bin/sh\nsleep 10\n`, build a
  `DriftAudit` with `executor_timeout_secs = 1`, call `audit.run(&mut
  ctx).await`, assert the result is `Err`, assert
  `format!("{err:#}")` contains both `drift_audit` and `timeout`.
- [x] 1.2 Same test — after `run` returns, read the log file at
  `ctx.log_writer.path()` and assert it contains the literal
  `kind: Err` AND `reason: timeout` from the
  `drift_audit_outcome` section the production path writes before
  the `Err`.

## 2. Architecture-consultative-audit timeout branch

- [x] 2.1 `run_returns_err_on_subprocess_timeout` (in
  `autocoder/src/audits/architecture_consultative.rs` tests module)
  — analogous shape: short timeout + `sleep 10` fake CLI, assert
  `Err` whose message contains `architecture_consultative` and
  `timeout`.
- [x] 2.2 Same test — read the audit log file and assert it contains
  the `kind: Err\nreason: timeout` section the production path
  writes for `architecture_consultative_outcome`.

## 3. Specs-writing timeout branch (via missing-tests)

- [x] 3.1 `run_returns_err_on_subprocess_timeout` (in
  `autocoder/src/audits/missing_tests.rs` tests module) — fake CLI
  that sleeps past the configured `executor_timeout_secs = 1`,
  assert `audit.run(&mut ctx).await` returns `Err`, assert the
  message contains `missing_tests_audit` and `timeout`.
- [x] 3.2 Same test — verify the workspace state is unchanged: no
  new directory under `openspec/changes/` (defense-in-depth:
  a timed-out CLI must not produce committed changes).
