# Tasks

## 1. Parser: recognize the wildcard sentinel

- [ ] 1.1 In the operator-command parser, for `clear-perma-stuck` AND `clear-revision`, accept a bare `*` in the change-slug position AND a bare `*` in the repo-substring position as the wildcard sentinel, recognized BEFORE the change-slug / repo-substring regex (so `*` is not rejected as malformed). Non-`*` arguments are sanitized unchanged. Add the one- and two-arg wildcard forms (`clear-<kind> *`, `clear-<kind> <repo> *`) to the command shapes; keep the exact two-arg form.

## 2. Handler: resolve the wildcard and sweep

- [ ] 2.1 In the `ClearPermaStuckMarker` / `ClearRevisionMarker` control-socket handling, resolve a `*` change target to "every marker of this kind in the resolved repo" and a `*` repo target to "every configured repository". Enumerate markers by scanning each repo's workspace for the kind's marker file. Preserve `clear-perma-stuck`'s accompanying `.ignore-for-queue.json` removal.
- [ ] 2.2 A per-repository read/remove failure is collected, not fatal: the sweep continues across remaining repos.

## 3. Reply: enumerate, never silent

- [ ] 3.1 The reply enumerates each repository AND each change/marker cleared; a repo (or the whole fleet) with no matching markers is reported as an explicit "nothing to clear"; per-repo failures are listed alongside successes.

## 4. Help text

- [ ] 4.1 Update the operator-command help text to document the wildcard forms for both verbs.

## 5. Tests

- [ ] 5.1 `clear-<kind> <repo> *` clears every marker of that kind in the repo and reports them; `clear-<kind> *` sweeps all repos.
- [ ] 5.2 `*` is accepted in the change/repo position (not rejected by the slug/repo regex); non-`*` args are still sanitized (a malformed slug is still rejected).
- [ ] 5.3 A repo with no markers yields an explicit "nothing to clear"; a per-repo failure is reported without aborting the sweep.
- [ ] 5.4 The exact-target form still clears exactly one named marker (unchanged behavior).
