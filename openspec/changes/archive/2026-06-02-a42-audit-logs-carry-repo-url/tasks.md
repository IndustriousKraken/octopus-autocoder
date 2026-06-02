# Implementation tasks

## 1. Thread `url` field into audit-module tracing sites

The pattern at every site: existing tracing call gains an extra `url = %ctx.repo.url` field (OR `url = %repo_url` when the function takes a `&str` URL directly rather than `&AuditContext`). The field name is exactly `url` for consistency with the polling-loop convention.

- [x] 1.1 `autocoder/src/audits/specs_writing.rs` — edit every `tracing::(warn|info|error)!` site inside `run_specs_writing_audit` AND its helpers. The function has `ctx.repo.url` available; pass `url = %ctx.repo.url` to every call. Approximate sites: validation-failure WARN at line ~210, validation-rejection WARN at ~256, dir-removal WARN at ~269, chatops-post-failed WARN at ~372.
- [x] 1.2 `autocoder/src/audits/missing_tests.rs` — same pattern. Function has access to the context the helper threads in. (No `warn|info|error` sites in this file; all spec-writing logging flows through the shared `run_specs_writing_audit` helper in 1.1, distinguished by `audit_type`.)
- [x] 1.3 `autocoder/src/audits/security_bug.rs` — same. (No `warn|info|error` sites; logging flows through `run_specs_writing_audit`.)
- [x] 1.4 `autocoder/src/audits/architecture_consultative.rs` — same. (The lone site is in the pure `parse_severity(&str)` helper with no `AuditContext`/URL in scope — annotated `// no-url:` per §2; the `workspace_unavailable_outcome` caller now threads `&ctx.repo.url`.)
- [x] 1.5 `autocoder/src/audits/drift.rs` — same. (Lone site in pure `parse_severity` — annotated `// no-url:`; `workspace_unavailable_outcome` caller threads `&ctx.repo.url`.)
- [x] 1.6 `autocoder/src/audits/brightline.rs` — same. (No `warn|info|error` sites; the `workspace_unavailable_outcome` caller threads `&ctx.repo.url`.)
- [x] 1.7 `autocoder/src/audits/documentation_audit.rs` — same. (Parse-failure WARN gains `url = %ctx.repo.url`; two `parse_severity` sites annotated `// no-url:`.)
- [x] 1.8 `autocoder/src/audits/mod.rs` — the shared helpers (`post_validation_exhausted_notification`, `post_proposal_created_notification`, etc.) take `repo_url: &str` directly; pass `url = %repo_url` to their internal tracing calls. (`discard_proposal_and_notify` + `post_proposal_created_notification` threaded; `workspace_unavailable_outcome` gained a `repo_url` param threaded from all 5 callers; the RAII `Drop` temp-file-cleanup WARN annotated `// no-url:`.)
- [x] 1.9 `autocoder/src/audits/scheduler.rs` — tracing calls within per-repo scheduling (where the loop iterates configured repos AND already has the repo URL bound) get `url = %repo.url`. Truly repo-agnostic scheduler tracing (audit registry initialization, no-audits-configured top-line) STAYS repo-agnostic. (13 of 15 sites already carried `url`; added it to the `.audit-state.json` exclude-registration WARN and the `stamp_audit_thread_state` WARN. No registry-init/no-audits messages exist at `warn|info|error` level.)

## 2. Annotation for genuinely repo-agnostic tracing calls

- [x] 2.1 Any tracing call site in `autocoder/src/audits/` that intentionally does NOT carry `url` SHALL have a `// no-url: <reason>` comment on the line immediately preceding the macro invocation. The reason text is short — e.g., `// no-url: registry init runs once at startup, no repo context yet`. (Annotated: pure `parse_severity` parsers in drift/architecture/documentation; RAII `Drop` cleanup in mod.rs; daemon-global `reload_from_disk`/`load_or_default` in state.rs; daemon-global `prune_stale_entries` in threads.rs; workspace-keyed ignore loader in brightline/ignore.rs.)
- [x] 2.2 The annotation is the regression test's escape hatch — a tracing call without `url =` AND without the comment fails the test in CI, so future contributors are forced to make the attribution choice explicit.

## 3. Regression test enforcing the convention

- [x] 3.1 Add an integration-style test at `autocoder/tests/audit_tracing_carries_url.rs` (OR extend an existing audit-tracing test file if one exists — check before creating). The test reads each `.rs` file under `autocoder/src/audits/` via `std::fs::read_to_string` AND scans for the regex `tracing::(warn|info|error)!`. For each match:
  - If the next ~10 lines contain `url =` → pass. (Implemented via paren-balanced macro-span scan.)
  - Else if the immediately-preceding line contains `// no-url:` → pass.
  - Else → fail with a diagnostic naming the file, line number, AND the offending macro line excerpt.
- [x] 3.2 The test SHALL be deterministic — no clock, no env mutation, no network. (Reads source files only.)
- [x] 3.3 The test SHALL produce a single combined failure summary (NOT first-failure-only), so an operator running it locally sees every offending site at once instead of fixing-then-re-running. (All offenders collected into one `assert!` listing; a self-test exercises the matcher on synthetic compliant/annotated/offending/`repo_url`-near-miss sources.)

## 4. Unit-level capture test

- [x] 4.1 In `autocoder/src/audits/specs_writing.rs`'s test module (OR a sibling test file), add a test that uses `tracing-subscriber`'s `tracing-test` layer (OR the existing in-tree span-capture helper if one is used elsewhere) to invoke the validation-failure path on a tiny fixture workspace AND asserts the captured WARN event's structured fields include `url: <fixture-repo-url>`. (Added `validation_failure_warn_carries_repo_url` to `missing_tests.rs`'s test module — a sibling that drives the same shared `run_specs_writing_audit` WARN — using the in-tree `#[tracing_test::traced_test]` helper already used elsewhere in the crate. It is parallel-safe via a process-global subscriber + per-test span scope, unlike a thread-local `set_default` subscriber which races on the callsite-`Interest` cache under parallel execution. The fixture validator emits its error via `printf` (no trailing newline) so the rendered WARN stays single-line and its `url=` field remains within the test's captured scope; the test asserts `logs_contain("url=<sentinel>")`.)
- [x] 4.2 The fixture's repo URL is a unique sentinel string (e.g., `https://example.invalid/sentinel-repo-a42`) so the assertion is unambiguous.

## 5. Acceptance gate

- [x] 5.1 `cargo test` passes for the autocoder crate, including the new regression AND capture tests. (New regression + capture tests pass; 2126/2127 pass. The 1 failure — `security_bug::tests::low_confidence_finding_filtering_explicit_in_prompt` — is a PRE-EXISTING prompt-drift failure verified to fail identically on the base commit with all a42 changes stashed; unrelated to this change.)
- [x] 5.2 `openspec validate a42-audit-logs-carry-repo-url --strict` passes.
- [ ] 5.3 Manual spot-check: `journalctl -u autocoder | grep "audit_type=missing_tests_audit" | head -5` shows `url=<repo-url>` in every line, as soon as a missing-tests run fires after the change deploys. (NOT performed inside the autocoder sandbox — requires a live, deployed daemon with `journalctl` access and a configured repository. This is a post-deploy operator verification step; the runtime behavior it checks is covered by the unit capture test in §4 and the source-presence regression test in §3.)
