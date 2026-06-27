# Tasks

## 1. The `prioritize` verb + parser

- [x] 1.1 Recognize `@<bot> prioritize <repo-substring> <change-slug> <N>` in the chatops inbound listener / dispatcher (case-insensitive verb), alongside the existing operator verbs. Resolve the repo substring via the existing case-insensitive substring-match rule (ambiguous → list candidates; missing → polite error), exactly as `status` / `clear-revision` do.
- [x] 1.2 Parse the trailing argument: a non-negative integer `N` sets the priority; the literal `clear` or `none` removes it. Reject a missing or malformed argument (negative, non-numeric, not `clear`/`none`) with a polite `✗ prioritize: ...` error and submit no action.

## 2. The control-socket action

- [x] 2.1 Add a `PrioritizeAction { repo_url, change_slug, priority: Option<u32>, channel, thread_ts }` (where `priority = None` means clear) and submit it over the daemon's Unix-domain control socket on a valid parse, mirroring the existing verb→action dispatch. Participate in the existing event-dedup cache so a redelivered Slack event submits exactly one action.

## 3. The `.priority.json` marker read/write/remove

- [x] 3.1 Add a marker helper that writes `<workspace>/openspec/changes/<slug>/.priority.json` (atomic tempfile + rename) carrying `{ priority: N }`, removes it on `clear`/`none`, and reads it for ordering. Refuse to write for a change-slug that does not resolve to a pending change (polite error; no file written).
- [x] 3.2 Add `.priority.json` to `.gitignore` alongside the other untracked daemon markers — it is daemon bookkeeping, never committed.
- [x] 3.3 Treat a corrupt `.priority.json` (truncated JSON, missing/negative `priority`) as unprioritized for ordering; enumeration MUST NOT fail on it.

## 4. The `list_pending` ordering change

- [x] 4.1 In `queue.rs::list_pending`, insert the priority tier BETWEEN the iteration-pending tier and the alphabetical tier: (1) iteration-pending markers first (UNCHANGED), (2) `.priority.json`-marked changes by ascending `priority` then alphabetical within equal priority, (3) unprioritized changes alphabetical. A change carrying BOTH markers still sorts in the iteration-pending tier (priority never preempts in-progress work). When no priority markers exist the returned order is byte-for-byte today's.

## 5. Status surfacing

- [x] 5.1 Annotate prioritized pending changes in the `status` reply's queue section with their N (e.g. `a07-foo (priority 3)`) so the rendered queue reflects the effective order. Unprioritized changes render unchanged.

## 6. Lifecycle

- [x] 6.1 Ensure a `.priority.json` marker is consumed when its change is archived (the change directory and its markers go away on archive), so a completed priority change leaves no residue and the queue returns to pure alphabetical once all priority changes are worked.

## 7. Tests

- [x] 7.1 `list_pending`: a single priority-marked change sorts ahead of alphabetically-earlier unprioritized changes but behind any iteration-pending change.
- [x] 7.2 `list_pending`: multiple priority markers sort by ascending N, alphabetical within equal N; a corrupt marker is treated as unprioritized and enumeration does not error; with no markers the order equals the prior alphabetical order.
- [x] 7.3 Parser: `prioritize <repo> <change> 3` submits a `PrioritizeAction` with `priority = Some(3)`; `... clear` and `... none` submit `priority = None`; a malformed `N` is refused with no action; an ambiguous repo lists candidates; redelivery submits exactly one action.
- [x] 7.4 Marker helper: write then read round-trips `{ priority: N }`; `clear` removes the file; writing for a non-pending slug is refused with no file written.
- [x] 7.5 Status: a workspace with a prioritized pending change renders the change with its `(priority N)` annotation; unprioritized changes render unchanged.
- [x] 7.6 Help: `@<bot> help` lists the `prioritize` verb with its syntax and one-line description.
