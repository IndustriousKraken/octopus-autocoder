## Why

When an audit logs a WARN, INFO, or ERROR, the structured fields name the audit type, the change slug, AND the attempt number — but NOT the repository the audit was running against. In a single-repo deployment this is fine: there's only one repo. In a multi-repo deployment (the production case for any operator running autocoder against more than a handful of projects), reading the daemon log to diagnose an audit failure becomes guesswork. Sample log line as observed in production:

```
WARN autocoder::audits::specs_writing: rejecting agent-produced change that failed `openspec validate --strict`: ...
  audit_type="missing_tests_audit" change=tests-edge-cases-in-compound-profile-queries attempt=0
```

Five identical-shaped WARNs in a row, all from the same audit. The operator running ten configured repositories has no way to tell which repo this came from short of:

- grepping ten workspace agent-branches for the change slug (slow, error-prone, AND the slug may not even exist on disk anymore — invalid proposals get deleted),
- waiting for the next chatops notification to land AND attribute by elimination (only works if exactly one repo is running audits at that moment),
- correlating to journalctl tail timestamps against other repo-specific log lines (works but cognitively expensive).

The fix is mechanical: every tracing call site in the audit modules SHALL carry a structured `url` (OR equivalent) field naming the repo. The `AuditContext` (which every audit run already holds) carries `ctx.repo.url`; threading it into the existing tracing macros is a one-line edit per site. No new struct fields. No new helpers. No behavior change beyond the log surface.

This is one of those changes that costs almost nothing AND silently improves operability for anyone running autocoder against more than one repo. The reason it wasn't done at audit-system construction time is `tracing` structured fields were added incrementally per-site AND the multi-repo case wasn't the dominant operator profile at the time. It is now.

## What Changes

**Every tracing call site in `autocoder/src/audits/` SHALL include a `url` field** carrying the repo URL the audit is running against, formatted via the existing `url = %ctx.repo.url` pattern that's already used in `polling_loop.rs` informational log lines. The field name is `url` (matching the polling-loop convention) NOT `repo_url` so operators reading logs see one consistent attribution key across audit + polling code paths.

**The pattern applies uniformly.** WARN, INFO, AND ERROR levels in audit modules ALL get the field — there's no level-based exclusion. The set of files covered:

- `autocoder/src/audits/specs_writing.rs`
- `autocoder/src/audits/missing_tests.rs`
- `autocoder/src/audits/security_bug.rs`
- `autocoder/src/audits/architecture_consultative.rs`
- `autocoder/src/audits/drift.rs`
- `autocoder/src/audits/brightline.rs`
- `autocoder/src/audits/documentation_audit.rs`
- `autocoder/src/audits/mod.rs` (the shared helpers — `post_validation_exhausted_notification`, `post_proposal_created_notification`, etc.)
- `autocoder/src/audits/scheduler.rs` (the scheduler's own tracing — when it logs about a specific repo's audit run)

Some shared helpers in `audits/mod.rs` don't take a `&AuditContext` directly (e.g., the notification posters take `repo_url: &str` already). Those continue to work — the requirement is that the `url` field appears in the structured-field set for tracing calls inside those helpers too. A helper that already has `repo_url` as a parameter SHALL pass it as `url = %repo_url` to its tracing calls.

**For scheduler-level tracing that fires BEFORE a per-repo audit context exists** (e.g., "audit registry initialized," "no audits configured for any repo"), the `url` field is omitted — there's no repo to attribute. A repo-agnostic message stays repo-agnostic. The requirement covers tracing calls that occur DURING OR AFTER a per-repo context is established.

**A regression test SHALL assert presence** by repo-grep'ing the audit modules for `tracing::(warn|info|error)!` AND verifying every match either (a) appears within a function that takes `&AuditContext` AND has `url =` in its field set, OR (b) is annotated with a `// no-url: <reason>` comment naming why the repo-agnostic case applies. The test fails on a new tracing site added without one of those two conditions, so the convention is self-enforcing for future audit code.

**No spec change to the audit framework's behavior.** The `Audit` trait, `AuditContext`, `AuditOutcome`, scheduler ordering, retry budgets, AND chatops notifications are unchanged. This is observability scaffolding only.

## Impact

- **Affected specs:**
  - `orchestrator-cli` — ADDED a new requirement defining the `url` field convention for audit-module tracing AND the regression test that enforces it.
- **Affected code:**
  - Every tracing call site in `autocoder/src/audits/` listed above gains a `url = %ctx.repo.url` (OR equivalent `url = %repo_url`) field. Estimated ~25-30 edits across ~10 files; each is a 1-line addition to the existing structured-field set.
  - New regression test (`autocoder/tests/audit_tracing_carries_url.rs` OR an extension to an existing audit-tracing test file) that performs the repo-grep.
- **Operator-visible behavior:**
  - Daemon log lines from any audit module carry the repo URL as a structured field. `journalctl -u autocoder | grep <repo-url>` AND `journalctl -u autocoder.service -o json | jq` both work to filter by repo.
  - No chatops change; chatops notifications already carry the repo URL.
- **Backward compatibility:** purely additive. Existing log-line filtering by `audit_type`, `change`, OR `attempt` continues to work.
- **Dependencies:** none. Independent of every other queued change. Can land in any order.
- **Acceptance:** `cargo test` passes; `openspec validate a42-audit-logs-carry-repo-url --strict` passes. Tests:
  - Repo-grep: each `tracing::(warn|info|error)!` site in `autocoder/src/audits/` either has `url =` in its field set OR is annotated with `// no-url: <reason>`.
  - Unit-level: a tracing-test layer (using `tracing-subscriber`'s `tracing-test` fixture, OR a hand-rolled span capture) for the existing `specs_writing` validation-failure path asserts the captured field set contains `url` matching the test fixture's repo URL.
