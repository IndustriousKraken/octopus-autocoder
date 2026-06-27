# Let an operator prioritize a pending change in the queue

## Why

The changes lane processes pending OpenSpec changes in a fixed order: a change
mid-iteration (carrying `.iteration-pending.json`) comes first, then everything
else strictly alphabetically by slug. That default is fine until an operator has
an opinion — a hot fix that should jump ahead of a dozen alphabetically-earlier
changes, or a low-value cleanup that should sink to the back. Today the only lever
is renaming the change directory to manipulate its alphabetical slot, which is
clumsy, rewrites history, and fights the daemon's own naming.

This change gives the operator a direct lever: a `prioritize` chatops verb that
stamps a numeric priority on a pending change. Prioritized changes are worked
ahead of the default alphabetical order (lower number = higher priority), WITHOUT
disturbing the one invariant that must hold — a change already mid-iteration is
never preempted, because abandoning in-progress work to start something else is
strictly worse than finishing it.

## What Changes

- A new operator verb `@<bot> prioritize <repo-substring> <change-slug> <N>` where
  `N` is a non-negative integer (lower N = higher priority). `@<bot> prioritize
  <repo> <change> clear` (or `none`) removes the priority. The verb mirrors the
  existing operator-verb conventions: case-insensitive verb, repo resolved by
  case-insensitive substring match (as `status` / `send it` / `clear-revision`
  already do), and a confirmation/ack reply.
- The verb writes (or removes) a per-change marker file `.priority.json` in the
  change directory `openspec/changes/<slug>/`, parallel to how
  `.iteration-pending.json` works — untracked daemon bookkeeping, gitignored like
  the other markers.
- The changes-lane queue ordering gains a middle tier. The pending order becomes:
  (1) iteration-pending / WIP changes first (UNCHANGED — a mid-iteration change
  always wins; priority never preempts in-progress work), then (2) priority-marked
  changes by ascending N (alphabetical within equal N), then (3) unprioritized
  changes alphabetical (the current default). When no priority markers exist the
  order is exactly today's.
- The `status` reply's queue section surfaces the effective priority ordering
  (prioritized changes annotated with their N) so the operator can see the order
  they just set.
- A priority marker is consumed when its change is archived (done); `clear`
  removes it on demand. Once all priority changes are worked the queue returns to
  pure alphabetical.

The issues and audits lanes are UNCHANGED — this is a changes-lane-only feature.

## Impact

- Affected specs: `openspec-queue-engine` (MODIFIED queue-ordering requirement —
  the priority tier is inserted between iteration-pending and alphabetical),
  `chatops-manager` (ADDED `prioritize` verb requirement AND ADDED status-queue
  priority-surfacing requirement).
- Affected code: `queue.rs::list_pending` ordering; the chatops inbound listener /
  dispatcher (new verb + parser + control-socket action); the `.priority.json`
  marker read/write/remove helper; the status queue-section formatter; the
  `.gitignore` entry for the new marker.
- This is the manual foundation for a future agent-prioritizer: an autonomous
  ranking agent could write the SAME `.priority.json` markers the operator writes
  here, and the queue ordering would honor them with no further change. That
  agent is explicitly out of scope.
- Reusing the priority pattern for the issues lane (and audits) is possible future
  work — the marker shape and ordering tier generalize — but is intentionally NOT
  done here to keep the contract focused on the changes lane.
- Independent change; the only existing requirement it modifies is the
  queue-engine ordering requirement, which no other in-flight change touches.
