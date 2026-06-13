## Context

Two architectural audits exist. `architecture_brightline` (orchestrator-cli
"Architecture-brightline audit") is pure-code, `WritePolicy::None`, and emits a
finding per threshold crossing across four metrics (file length, function
length, duplicate signature, duplicate body), with `.brightline-ignore`
suppression for the duplicate metrics. `architecture_consultative`
(orchestrator-cli "Architecture consultative audit" + "Consultative audit
prioritizes oversized, low-cohesion code") invokes the agent CLI read-only and
returns 0–5 anchored questions via `submit_findings`. Both ship findings as
`AuditOutcome::Reported`; an operator acts on them with `@<bot> send it`, which
runs the audit-reply triage. That triage (orchestrator-cli "Completed triage
splits into one or two PRs by content path") keeps only paths under
`openspec/changes/<slug>/` and reverts everything else, so today every triage
output is a spec change.

The two slugs are referenced across roughly a dozen requirements in three
capability specs. The selector logic this redesign keeps — whole-file line
counting (`check_file_size`, `contents.lines().count()`) — is the one brightline
measurement that is correct in every language.

## Goals / Non-Goals

**Goals:**
- One audit that applies judgment and returns actionable refactor
  recommendations, bounded in number, grounded in the project's language and
  patterns.
- Metrics used only to choose where to look, never emitted as findings.
- Behavior-preserving refactors routed to the issues lane; specs reserved for
  genuine contract or capability changes.
- Remove the structural pressure that turned a heuristic into a canonical
  requirement.

**Non-Goals:**
- Detecting cross-file rot (parallel implementations, dead code, discoverability
  failure) — that is the deferred deep-architecture audit.
- Per-language block-structure parsing. The redesign removes the function-length
  metric rather than special-case each language's blocks.
- Changing the `send it` / audit-thread mechanism, the audit cadence framework,
  or the issues-lane walker.

## Decisions

### D1 — Metrics are selectors, not findings
The advisor selects candidates with a cheap, deterministic, language-agnostic
signal: whole-file line count, taking the longest N files that exceed a pain
threshold (default tunable; N small — the worst handful, not everything over a
line). The raw count is internal; it never becomes a finding. This kills the
hundreds-of-findings volume at the source and removes the broken function-length
metric entirely (a long function lives in a long file, which the file selector
already surfaces; the rare god-function in an otherwise-short file is caught by
the judgment pass when it reads the selected file's neighbours, not by a second
noisy metric).

### D2 — One audit, judgment-based, replacing both
`architecture_advisor` invokes the agent CLI read-only (`WritePolicy::None`,
`requires_head_change = true`) on the selected candidates. For each it reads the
file and judges: is a refactor warranted, what kind of problem is it, and what
is the concrete recommendation, expressed in terms of this project's actual
language/architecture/patterns. This subsumes the consultative audit's judgment
while fixing its two faults — it is bounded by the selector (not "scan the whole
tree and free-associate") and it must end in a recommendation, not a question.

### D3 — Bounded, actionable advisory output; evidenced clean run
Output is a ranked list capped at a small number (carry the consultative audit's
0–5 cap), each finding stating what is wrong, why it matters, and the
recommended action with its anchor. Tone is specific and professional — no
snark, no generic best-practice lectures. A run that finds nothing worth
refactoring returns an evidenced "no action recommended" carrying a one-line note
of what it examined, not a silent empty outcome (consistent with the
fail-closed-and-report direction; the audit looked, and says so).

### D4 — Refactors route to the issues lane by default
The audit-reply triage gains the issues lane as a valid output. The
"Completed triage splits…" keep-rule is widened: the triage keeps paths under
`openspec/changes/<slug>/` OR `issues/<slug>/` — whichever lane it wrote for this
run — and reverts everything else by the same per-path strategy. The triage
prompt's routing rule: a behavior-preserving refactor → an issue
(`issues/<slug>/`: `issue.md` + `tasks.md`, no `specs/`); a change/spec ONLY when
the refactor requires altering an observable contract or surfaces a new
capability decision; never a spec to codify a metric or standard. Default to
issue for architectural findings. The issue queues on promotion and an
implementer performs the decomposition through the standard pipeline; the
operator reviews the PR.

### D5 — No audit metric becomes a canonical requirement
An explicit guard, stated as an orchestrator-cli requirement on audit triage and
reflected in the triage prompt: triage SHALL NOT author a canonical requirement
whose content is an audit's own selection metric (a size threshold, a count, a
duplication budget). Such thresholds are heuristics for where to look, not
contracts. This generalizes beyond the advisor to every metric-style audit and
removes the Goodhart pressure that produced the `code-organization` `SHALL`
budgets.

### D6 — Remove `.brightline-ignore` with its only consumer
The ignore file suppresses only duplicate-signature/body findings. Removing those
metrics leaves it with nothing to suppress, so it is removed along with them.
Whether a successor ignore mechanism returns is deferred to the deep-architecture
work (and the existing TODO that proposes extending the same pattern to the
contradiction gates); this change does not pre-build one.

### D7 — A new slug rather than reusing either name
The behavior change is large enough that reusing `architecture_brightline` or
`architecture_consultative` would mislead. The new slug `architecture_advisor`
replaces both in the registered-audit list. Config keys, the validator slug
list, chatops substring matching, the README table, and `config.example.yaml`
move to the one slug.

## Risks / Trade-offs

- **Large canonical surface.** Removing two audits touches ~12 requirements
  across three specs. Mitigated by handling the load-bearing requirements as
  deltas and enumerating the mechanical name-swaps as tasks; the change is a
  redesign, so the breadth is inherent.
- **Selector misses a god-function in a short file.** A 350-line function in a
  400-line file passes a file-length selector. Accepted: it is still a refactor
  of that one file, the judgment pass surfaces it once that file is selected for
  a neighbouring reason, and the alternative (a separate, noisy, often-wrong
  function-length metric) costs more than it saves. If this proves real,
  add a cheap secondary selector (largest single function per file) — still a
  selector, still not a finding.
- **Issues-lane routing widens the triage write-scope.** Permitting
  `issues/<slug>/` output means the keep/revert path must treat two subtrees as
  in-scope. The revert mechanics are unchanged per path; only the in-scope set
  grows by one well-defined directory.
- **Judgment variance.** An LLM judgment pass is less reproducible than a line
  count. That is the point — the line count was reproducibly unhelpful. The cap,
  the anchor requirement, and human arbitration at `send it` bound the downside.
