## Context

Startup fork setup (`ensure_forks_exist_with` in `autocoder/src/cli/run.rs`) treats any 2xx from `POST /repos/{owner}/{repo}/forks` as "fork created" and discards the response body (`create_fork_at` in `autocoder/src/forge/github.rs`). GitHub's fork endpoint is idempotent: if the PAT's account already has a fork of the upstream, it returns 2xx with the *existing* fork's metadata — which, after an upstream rename, still carries the old name. The daemon then polls the derived (new-name) fork URL for 60 seconds, fails, and reports a misleading "not reachable within 60s" timeout for a condition that is deterministic and will recur on every restart. The alert's remedy hint also says bare `reload`, which is not an invocable command.

## Goals / Non-Goals

**Goals:**
- Detect the returned-fork-differs-from-expected case at creation time and fail that repository's setup immediately with a cause that tells the operator exactly what to do (rename the fork).
- Make the fork-setup alert's remedy hint name the real command (`autocoder reload` on the daemon host).

**Non-Goals:**
- Auto-renaming or deleting the mismatched fork (destructive; operator's call).
- Retry/backoff for transient startup failures (tracked separately by the existing TODO on `repo_passes_startup_check`).
- Any change to GitLab or direct-push (no `fork_owner`) modes — fork setup is GitHub-specific today.

## Decisions

- **`create_fork` returns the fork identity instead of `()`.** `create_fork_at` parses the 2xx body's `full_name` and returns `Option<String>` (`None` when the body yields no identity). HTTP/JSON handling stays in the forge module; the match/mismatch *decision* stays in `ensure_forks_exist_with`, preserving the existing boundary where the `ForkOps` trait lets startup tests script outcomes without network.
- **Compare `full_name` case-insensitively against the owner/name derived from the fork URL.** The expected pair is parsed from the derived fork URL with the same URL-parsing helper used for upstreams. GitHub treats repo names case-insensitively, so a case-only difference is not a mismatch.
- **Unparseable identity falls back to the reachability poll.** `git ls-remote` remains the ground truth; the identity check is a fast-path diagnostic only. A GitHub response-shape change must never fail a fork setup that would otherwise succeed. (This is infrastructure setup, not a control-plane verdict gate, so the fail-closed gatekeeper canon does not apply — no verdict is being synthesized.)
- **Mismatch skips the poll entirely.** Polling a URL that names a repository GitHub just told us does not exist under that name cannot succeed; waiting 60s only delays and obscures the real cause.

## Risks / Trade-offs

- [GitHub could fork to an alternate name legitimately (name collision → `repo-1`)] → Same handling is correct: the derived fork URL will never be reachable, and the precise cause (actual vs expected name) is exactly what the operator needs.
- [Response body shape drift] → Mitigated by the fall-back-to-poll decision; behavior degrades to today's, never worse.
- [Trait signature change ripples through startup tests] → The scripted `ForkOps` test impls are colocated in `cli/run.rs` tests; the change is mechanical and caught at compile time.
