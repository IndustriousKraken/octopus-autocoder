# Survival and provenance analysis for past PRs and commits

## Why

Reviewing a past PR or commit is only half the value. The other half — the part
that makes reviewing OLD work worthwhile — is knowing which of its changes still
survive in the current code. Then an operator can review a long-past, untrusted
change and spec a fix for the problems that are still live, instead of chasing
code that was already overwritten. The inverse is just as useful: a problem found
in current code needs to be traced to the commit (and PR) that introduced it, so
the operator can decide between rolling it back (recent) and spec/issue-ing a fix
(older). `git blame` plus `git log <X>..HEAD` are exactly the tools for this.

## What Changes

- A new `orchestrator-cli` requirement adds `survives <repo> <pr N | commit sha>`
  (CLI + chatops, read-only): for each file the target touched, a cheap
  pre-filter (`git log <target>..HEAD -- <file>` empty → fully survives) and, for
  later-modified files, `git blame` at HEAD to keep the target's added lines that
  still attribute to it. The report summarizes per-file and overall survival and
  states its boundary plainly: it detects VERBATIM survival — it under-reports
  (misses surviving-but-edited lines) and never over-reports (a surviving line is
  the target's exact text). The surviving regions are consumable as the focus of
  an on-demand review.
- A second `orchestrator-cli` requirement adds the inverse `blame <repo> <path>
  <line>[-line]` (CLI + chatops, read-only): the introducing commit (short SHA,
  subject, date) for current line(s), plus the PR when discoverable.

## Impact

- Affected specs: `orchestrator-cli` (ADD survival analysis AND provenance
  lookup).
- Affected code: a `git blame` helper (we have `git log`/`rev-list` but no blame
  yet); a range→target resolver (PR/commit → commit-set); the survival resolver
  (per-file pre-filter + line-level blame intersection); the two verbs +
  control-socket actions + CLI subcommands; PR association for a commit via the
  forge.
- Read-only analysis. Pairs with the on-demand review, commit-log (`log`), and
  code-rollback changes: list → review → survives/blame → roll back (recent) or
  spec/issue (older). The verbatim-vs-semantic boundary is stated in the output
  so "N lines survive" is read correctly.
