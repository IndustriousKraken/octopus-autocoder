## ADDED Requirements

### Requirement: Survival analysis — what of a past PR or commit is still live
The orchestrator SHALL provide an analysis that reports which of a past pull request's OR commit's changes still survive verbatim in the current base-branch tree, so an operator can review long-past work that is still live AND spec a fix for surviving problems. It SHALL be available BOTH as a CLI subcommand AND as a chatops verb (`@<bot> survives <repo-substring> <pr <N> | commit <sha>>`), resolving the repository by the same selector rule the other operator commands use. It SHALL be read-only.

The analysis SHALL resolve the target to a commit (or commit-set: a single commit, OR a PR's commits / squash-merge commit) AND, for each file that target modified, determine which of the lines it ADDED still attribute to that target at the current `HEAD`:

- As a cheap per-file pre-filter, a file the target touched that NO later commit has touched (`git log <target>..HEAD -- <file>` is empty) SHALL be reported as fully surviving — every hunk the target made there is still present, without line-level blame.
- For a file later modified, the analysis SHALL use `git blame` at `HEAD` to keep the target's added lines that STILL attribute to the target; lines later overwritten attribute to a newer commit AND are reported as not surviving.

The report SHALL summarize, per file, how much of the target survives (e.g. fully / partially with the surviving line regions / not at all) AND an overall count, newest-relevant first, naming the target.

The analysis SHALL state its boundary plainly in its output: it detects VERBATIM survival, not semantic survival. Because `git blame` attributes a line to the LAST commit that touched it, a line the target introduced that was later reformatted, renamed, or moved attributes to the newer commit AND is reported as not surviving even if its substance persists. The analysis therefore UNDER-reports survival (it may miss surviving-but-edited lines) AND never over-reports (a line reported as surviving is the target's exact text). Move/copy detection (`git blame -M -C`) MAY be applied to recover relocated lines; it is heuristic AND does not change the verbatim-vs-semantic boundary.

The surviving regions SHALL be consumable as the focus of an on-demand review (the operator can review only what is still live) — the analysis names the surviving files AND line regions in a form the review command can target.

#### Scenario: A file untouched since the target survives fully via the cheap pre-filter
- **WHEN** the target modified a file AND `git log <target>..HEAD -- <file>` is empty
- **THEN** the file is reported as fully surviving without running line-level blame

#### Scenario: A later-modified file is resolved line-by-line via blame
- **WHEN** the target modified a file that a later commit also touched
- **THEN** the analysis reports the target's added lines that still attribute to the target at `HEAD` as surviving, AND those overwritten by a newer commit as not surviving

#### Scenario: The report states the verbatim-survival boundary
- **WHEN** the analysis produces its report
- **THEN** the report states that it detects verbatim survival (under-reports survival, never over-reports) — a line reported as surviving is the target's exact text
- **AND** the per-file/overall survival counts are shown, naming the target

#### Scenario: Survival output can focus an on-demand review
- **WHEN** an operator follows a survival report with an on-demand review
- **THEN** the surviving files AND line regions are available as the review target so only still-live code is reviewed

### Requirement: Provenance lookup — where a line was introduced
The orchestrator SHALL provide the inverse of survival analysis: given a file AND a line (or line range) in the current tree, report the commit that last introduced it AND the pull request that commit belongs to, so a problem found in current code can be traced to its origin AND the operator can decide between rolling back (recent) AND spec/issue-ing a fix (older). It SHALL be available as a CLI subcommand AND a chatops verb (`@<bot> blame <repo-substring> <path> <line>[-<line>]`), resolve the repository by the standard selector rule, AND be read-only.

The lookup SHALL run `git blame` for the named line(s) at `HEAD`, report each line's introducing commit (short SHA, subject, date), AND — when the commit can be associated with a pull request (e.g. a merge commit, OR a commit whose PR is discoverable via the forge) — name that PR. When no PR association is found, the commit alone is reported.

#### Scenario: A current line is traced to its introducing commit
- **WHEN** an operator requests provenance for a file AND line range
- **THEN** the response names, per line, the commit that last introduced it (short SHA, subject, date)

#### Scenario: The introducing commit's PR is named when discoverable
- **WHEN** the introducing commit can be associated with a pull request
- **THEN** the response names that PR alongside the commit
- **AND** when no PR association is found, the commit alone is reported (no fabricated PR)
