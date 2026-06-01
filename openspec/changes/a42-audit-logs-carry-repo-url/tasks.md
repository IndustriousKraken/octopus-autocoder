# Implementation tasks

## 1. Thread `url` field into audit-module tracing sites

The pattern at every site: existing tracing call gains an extra `url = %ctx.repo.url` field (OR `url = %repo_url` when the function takes a `&str` URL directly rather than `&AuditContext`). The field name is exactly `url` for consistency with the polling-loop convention.

- [ ] 1.1 `autocoder/src/audits/specs_writing.rs` — edit every `tracing::(warn|info|error)!` site inside `run_specs_writing_audit` AND its helpers. The function has `ctx.repo.url` available; pass `url = %ctx.repo.url` to every call. Approximate sites: validation-failure WARN at line ~210, validation-rejection WARN at ~256, dir-removal WARN at ~269, chatops-post-failed WARN at ~372.
- [ ] 1.2 `autocoder/src/audits/missing_tests.rs` — same pattern. Function has access to the context the helper threads in.
- [ ] 1.3 `autocoder/src/audits/security_bug.rs` — same.
- [ ] 1.4 `autocoder/src/audits/architecture_consultative.rs` — same.
- [ ] 1.5 `autocoder/src/audits/drift.rs` — same.
- [ ] 1.6 `autocoder/src/audits/brightline.rs` — same.
- [ ] 1.7 `autocoder/src/audits/documentation_audit.rs` — same.
- [ ] 1.8 `autocoder/src/audits/mod.rs` — the shared helpers (`post_validation_exhausted_notification`, `post_proposal_created_notification`, etc.) take `repo_url: &str` directly; pass `url = %repo_url` to their internal tracing calls.
- [ ] 1.9 `autocoder/src/audits/scheduler.rs` — tracing calls within per-repo scheduling (where the loop iterates configured repos AND already has the repo URL bound) get `url = %repo.url`. Truly repo-agnostic scheduler tracing (audit registry initialization, no-audits-configured top-line) STAYS repo-agnostic.

## 2. Annotation for genuinely repo-agnostic tracing calls

- [ ] 2.1 Any tracing call site in `autocoder/src/audits/` that intentionally does NOT carry `url` SHALL have a `// no-url: <reason>` comment on the line immediately preceding the macro invocation. The reason text is short — e.g., `// no-url: registry init runs once at startup, no repo context yet`.
- [ ] 2.2 The annotation is the regression test's escape hatch — a tracing call without `url =` AND without the comment fails the test in CI, so future contributors are forced to make the attribution choice explicit.

## 3. Regression test enforcing the convention

- [ ] 3.1 Add an integration-style test at `autocoder/tests/audit_tracing_carries_url.rs` (OR extend an existing audit-tracing test file if one exists — check before creating). The test reads each `.rs` file under `autocoder/src/audits/` via `std::fs::read_to_string` AND scans for the regex `tracing::(warn|info|error)!`. For each match:
  - If the next ~10 lines contain `url =` → pass.
  - Else if the immediately-preceding line contains `// no-url:` → pass.
  - Else → fail with a diagnostic naming the file, line number, AND the offending macro line excerpt.
- [ ] 3.2 The test SHALL be deterministic — no clock, no env mutation, no network.
- [ ] 3.3 The test SHALL produce a single combined failure summary (NOT first-failure-only), so an operator running it locally sees every offending site at once instead of fixing-then-re-running.

## 4. Unit-level capture test

- [ ] 4.1 In `autocoder/src/audits/specs_writing.rs`'s test module (OR a sibling test file), add a test that uses `tracing-subscriber`'s `tracing-test` layer (OR the existing in-tree span-capture helper if one is used elsewhere) to invoke the validation-failure path on a tiny fixture workspace AND asserts the captured WARN event's structured fields include `url: <fixture-repo-url>`.
- [ ] 4.2 The fixture's repo URL is a unique sentinel string (e.g., `https://example.invalid/sentinel-repo-a42`) so the assertion is unambiguous.

## 5. Acceptance gate

- [ ] 5.1 `cargo test` passes for the autocoder crate, including the new regression AND capture tests.
- [ ] 5.2 `openspec validate a42-audit-logs-carry-repo-url --strict` passes.
- [ ] 5.3 Manual spot-check: `journalctl -u autocoder | grep "audit_type=missing_tests_audit" | head -5` shows `url=<repo-url>` in every line, as soon as a missing-tests run fires after the change deploys.
