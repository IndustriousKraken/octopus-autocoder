# Design — app-under-test-e2e

## Why the daemon owns the application lifecycle, not the agent

The agent could start the app itself with `Bash`. It should not.

- **Teardown.** A dev server started by the agent is a grandchild of the session. The session is spawned in its own process group and killed as a group on timeout, which mostly works — but "mostly" leaks listeners on a long-lived production daemon. Daemon-owned start/stop makes teardown a guarantee with an explicit code path, including on panic, timeout, and SIGTERM.
- **Port collisions.** One tokio task per repository means two repositories can be mid-pass simultaneously. Both would try port 3000. The daemon allocates an ephemeral port per pass and hands the resolved URL to the session.
- **Sandbox surface.** Daemon ownership means no relaxation of `disallowed_bash_patterns`. The readiness probe runs in daemon code, not through the agent's shell, so `curl:*` stays denied.
- **Trust.** The start command and readiness probe live in operator config. An agent that could rewrite the readiness probe could make any pass look green — the same reason canon and the gates are not agent-editable.

The agent may still restart the app during a session using the configured start command; the restarted process is inside the session's process group and is reaped with it.

## Why the exit code is the oracle and screenshots are not

The tempting design is "the agent looks at a screenshot and decides." Rejected:

- **CLI variance.** Three CLI strategies are supported (`claude`, `opencode`, `agy`) and their image-reading abilities differ. A contract that depends on vision silently degrades on two of three backends.
- **Self-reference.** An agent judging its own screenshot is the same failure this change exists to fix, moved one level down.

So the contract is the e2e command's exit code. Screenshots remain valuable — the agent takes them, reads them where its CLI supports it, and uses them to debug its own loop — but they are never the pass/fail signal. This also means the feature works for non-browser applications with no additional design: a CLI or API application declares an e2e command and gets the same treatment, which is the reason the config block is named `app_under_test` rather than anything browser-specific.

Playwright is the expected tool for browser applications (deterministic, scriptable, hermetic browser contexts, no persistent profile or credentials, and usable by non-Claude agents). Per the binding-contract-vs-guidance rule, that is guidance recorded here, not contract: the spec constrains only that a declared e2e command runs and its exit code is authoritative.

## Red-green replay

A test written in the same session as the feature can pass vacuously — asserting on a selector the agent invented, or never mounting the component. The check is mechanical, so it does not need an LLM:

1. Create a scratch git worktree at the pass's base commit.
2. Overlay only the new or modified e2e test files from the agent branch.
3. Start the app from that tree and run the e2e command.
4. The test **must fail**. A test that passes without the change is not evidence.

Overlaying tests onto the base tree (rather than reverting the implementation) keeps the operation read-only with respect to the agent branch and avoids stash/reset fragility. The replay starts a second application instance, so it takes its own ephemeral port allocation — the pass's instance may still be bound, and two instances cannot share a port.

A pass-against-base is genuinely ambiguous: the test may be vacuous, or the behavior may have already existed (the self-heal-already-implemented path is a real, already-handled case in this codebase). The change therefore does not discard work or hold the change. It drafts the PR and names the finding, which is the existing mechanism for "a human must look at this before it merges" — see the reviewer-`Block` draft path. The operator resolves the ambiguity; the machine refuses to call it verified.

## Degradation posture

This is opt-in per repository, so the fail-closed reasoning that governs the gates does not transfer directly: a missing e2e toolchain must not stop unrelated work.

- No `app_under_test` block → today's behavior exactly, no new code paths.
- Block present but toolchain missing → that repository's e2e feature is disabled with a startup WARN and a `doctor` finding. The daemon starts; other repositories are unaffected.
- App fails to start or never becomes ready → the pass proceeds without an app, the prompt block is omitted, and the PR body records that verification did not run. This is deliberately not a hold: a broken dev server is an environment condition, and blocking the queue on it would trade one churn problem for a worse one.

The distinction from the gates is that the gates decide whether a change is *allowed to proceed*; this feature decides whether a change can be *reported as verified*. An unverified change is visibly unverified, which is the honest outcome and the one that lets a human intervene.

## Bounds

An e2e loop is expensive in wall clock and tokens, and this repository has a documented token-burn history. Every new operation is bounded: application startup has a readiness timeout, the e2e command has its own timeout, and the red-green replay is skipped entirely when no e2e test file changed (the common case for a backend-only change). Timeouts are operator-configurable and clamped at load, following the existing numeric-knob precedent.
