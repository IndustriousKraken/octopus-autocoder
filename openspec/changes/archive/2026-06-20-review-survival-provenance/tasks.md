# Tasks

## 1. Git primitives

- [x] 1.1 Add a `git blame` helper that returns, per line of a file at a ref, the introducing commit (SHA, subject, date). Support a line-range subset AND optional `-M -C` move/copy detection.
- [x] 1.2 Add (or reuse) a per-file "touched since" query: `git log <target>..HEAD -- <file>` non-empty iff a later commit touched the file.

## 2. Target resolution

- [x] 2.1 Resolve a `pr <N>` target to its commit-set (the PR's commits, or its squash-merge commit) and a `commit <sha>` target to that commit, against the base branch. Collect the files each target modified and the lines it added.

## 3. Survival resolver

- [x] 3.1 For each file the target touched: if `git log <target>..HEAD -- <file>` is empty, mark fully surviving (no blame). Otherwise run `git blame` at HEAD and keep the target's added lines that still attribute to the target; the rest are not surviving.
- [x] 3.2 Produce a report: per-file survival (fully / partial with surviving line regions / none) + overall counts, naming the target. Emit the surviving files + line regions in a form the on-demand review command can target.
- [x] 3.3 The report states the verbatim-survival boundary (under-reports, never over-reports).

## 4. Provenance lookup

- [x] 4.1 `blame <repo> <path> <line>[-line]`: run `git blame` at HEAD for the line(s), report each line's introducing commit (SHA, subject, date). Associate the commit with a PR when discoverable via the forge; otherwise report the commit alone (no fabricated PR).

## 5. Operator surface

- [x] 5.1 Add `survives` and `blame` chatops verbs + CLI subcommands, using the existing repo-selector resolution. Both read-only.

## 6. Tests

- [x] 6.1 A file untouched since the target is reported fully surviving via the pre-filter (no blame invoked for it).
- [x] 6.2 A later-modified file: the target's still-attributed lines are surviving; overwritten lines are not.
- [x] 6.3 The report states the verbatim boundary and never reports a line as surviving unless blame attributes it to the target.
- [x] 6.4 Provenance: a current line maps to its introducing commit; the PR is named when discoverable and omitted (commit-only) when not.
- [x] 6.5 Both commands are read-only (no branch/workspace/marker mutation).
