# Tasks

## 1. Parser: recognize the wildcard sentinel

- [x] 1.1 In the operator-command parser, for `clear-perma-stuck` AND `clear-revision`, accept a bare `*` in the change-slug position AND a bare `*` in the repo-substring position as the wildcard sentinel, recognized BEFORE the change-slug / repo-substring regex (so `*` is not rejected as malformed). Non-`*` arguments are sanitized unchanged. Add the one- and two-arg wildcard forms (`clear-<kind> *`, `clear-<kind> <repo> *`) to the command shapes; keep the exact two-arg form.

## 2. Handler: intercept the wildcard BEFORE the single-slug resolver, then sweep

- [x] 2.1 In `handle_clear_perma_stuck` / `handle_clear_revision` (control_socket.rs), branch on `change == "*"` BEFORE the existing `queue::resolve_change_prefix(...)` call — the load-bearing call site. A `*` MUST NOT be passed to `resolve_change_prefix` (a literal `*` matches no directory prefix → `NoMatch` → a silent "nothing to clear"). For `*`, take the sweep path: enumerate the kind's marker directories directly. `resolve_change_prefix` continues to handle only non-`*` (single-target) values, unchanged.
- [x] 2.2 Resolve a `*` change target to "every marker of this kind in the resolved repo" and a `*` repo target to "every configured repository". Enumerate markers by scanning each repo's workspace for the kind's marker file. Preserve `clear-perma-stuck`'s accompanying `.ignore-for-queue.json` removal.
- [x] 2.3 A per-repository read/remove failure is collected, not fatal: the sweep continues across remaining repos.

## 3. Reply: enumerate, never silent

- [x] 3.1 The reply enumerates each repository AND each change/marker cleared; a repo (or the whole fleet) with no matching markers is reported as an explicit "nothing to clear"; per-repo failures are listed alongside successes.

## 4. Help text

- [x] 4.1 Update the operator-command help text to document the wildcard forms for both verbs.

## 5. Tests

- [x] 5.1 `clear-<kind> <repo> *` clears every marker of that kind in the repo and reports them; `clear-<kind> *` sweeps all repos.
- [x] 5.2 `*` is accepted in the change/repo position (not rejected by the slug/repo regex); non-`*` args are still sanitized (a malformed slug is still rejected).
- [x] 5.3 A repo with no markers yields an explicit "nothing to clear"; a per-repo failure is reported without aborting the sweep.
- [x] 5.4 The exact-target form still clears exactly one named marker (unchanged behavior).
- [x] 5.5 A `*` target does NOT reach `resolve_change_prefix` (assert the wildcard branch is taken before resolution — e.g. a `*` sweep succeeds in a workspace where prefix resolution of `*` would return `NoMatch`).
