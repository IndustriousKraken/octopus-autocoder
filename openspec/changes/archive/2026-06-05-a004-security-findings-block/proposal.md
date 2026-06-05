## Why

In PR #95 the reviewer correctly *found* a plaintext-API-key-leak risk (`opencode.json` carrying a key into a committable workspace file) but rated it **`Concerns`**, not **`Block`**. A `Concerns` verdict doesn't draft the PR and doesn't stop a merge — so "you might ship an API key to a public repo" rode through as advisory. A credential leak is the single most stop-the-line thing a reviewer can find; rating it `Concerns` is the verdict under-classifying a security-critical issue.

This is also the root of the "it passed but still revised" confusion: the surprise wasn't the auto-revise, it was that a security finding produced a soft verdict. Fix the verdict and it makes sense again. (It also makes a05's `auto_revise: block` default sufficient — security gets auto-fixed because it now Blocks.)

## What Changes

**Security-critical findings yield `Block`.** The reviewer SHALL return a `Block` verdict for security-critical findings — credential/secret leakage (a key or secret written where it could be committed or otherwise exposed), hardcoded secrets, and injection vulnerabilities — never `Concerns` or `Pass`. Two layers:

- **Prompt rule:** the reviewer prompt explicitly instructs that credential leakage, secret exposure, and injection are `Block`-class, not `Concerns`.
- **Code-level safety net:** when the reviewer's own output flags a finding as a secret/credential/key exposure but returns a non-`Block` verdict, the daemon escalates the verdict to `Block` — so a mis-classifying model can't downgrade a credential leak to advisory.

## Impact

- **Affected specs:** `code-reviewer` — ADD `Security-critical findings yield a Block verdict`.
- **Affected code:** the reviewer prompt (`prompts/code-review-default.md` / the configured override) gains the security-severity rule; the verdict-handling path gains the escalation safety net keyed on the reviewer's own security-finding signal.
- **Operator-visible behavior:** a review that finds a credential leak / secret / injection drafts the PR (`Block`) instead of passing it through as `Concerns`. Non-security findings are unaffected.
- **Dependencies:** none. Pairs with a005 (`auto_revise: block`): together, security findings both Block AND auto-fix, while style `Concerns` stay advisory.
- **Acceptance:** `cargo test` passes; `cargo clippy --all-targets -- -D warnings` is clean; `openspec validate a004-security-findings-block --strict` passes. Tests: a reviewer result flagging a credential/secret leak with a non-`Block` verdict is escalated to `Block` (assert the verdict, not message text); a non-security `Concerns` finding is left as `Concerns`.
