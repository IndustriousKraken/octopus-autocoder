# Redesign the architecture audits into one judgment-based advisor

## Why

The goal of an architectural audit is to make code get cleaned up and refactored
with some regularity — the thing a human does naturally when a codebase starts
to hurt, but that an agent may not do on its own. The two audits that exist today do not serve that goal.

`architecture_brightline` is a pure-metric audit: it counts file lines, function
lines, duplicate signatures, and duplicate bodies, and emits each threshold
crossing as a finding. In practice it produces hundreds of findings per run, it
cannot generalize until that pile is dumped on a triage agent, and the findings
themselves are often wrong:

- The function-length metric runs a brace `{ }` matcher (`find_function_end`)
  over every scanned language. For an indentation-delimited language (Python,
  and likewise Ruby, Lua, Haskell) there is no closing brace to find, so the
  matcher walks past the function to the first unrelated `{ … }` later in the
  file and reports a wildly inflated span. A five-line method is reported as
  hundreds of lines. Special-casing each language's block structure is an open-
  ended treadmill.
- File length and function length are redundant signals for one fact. A file's
  length is the sum of what it contains, so a long file is long because its
  functions are too long or there are too many of them — either way the remedy
  is a refactor of that same file. Flagging both produces double the noise to
  make a single point: "someone should look at this file."
- The duplicate-signature metric keys on the interface, so it trips on
  language idioms (`Default::default`, `Display::fmt`, per-binary `main`,
  framework `setUp`/`tearDown`) and pushes the burden of separating idiom from
  real duplication onto the operator via a hand-maintained `.brightline-ignore`.

`architecture_consultative` has the opposite failure. It applies judgment but
stops at diagnosis: it returns evocative questions ("did this file quietly
become the junk drawer?") with no recommendation. The operator still has to open
the file and work out what, if anything, to do — which they could have done
without the audit. It is a vibe, not an action.

So one audit emits data and asks a downstream agent to manufacture judgment from
it; the other emits judgment but no action. Neither delivers what the goal
needs: an informed recommendation about whether a specific file should be
refactored and how, grounded in this project's language, patterns, and
architecture. (Only one architectural audit was ever asked for; the second was
agent-proposed — the same proliferation reflex that, in a recent run, led a
triage agent to promote the brightline thresholds into canonical `SHALL`
requirements, turning a noisy heuristic into law.)

## What Changes

Replace both audits with a single advisory audit, `architecture_advisor`.

- **Metrics become selectors, never findings.** A cheap, language-agnostic
  signal (whole-file line count — `wc -l`, correct in every language; optionally
  git-churn later) picks a small set of candidate files: the longest few over a
  pain threshold, not every file over a line. The raw count is never emitted as
  a finding; it only decides where to point judgment. (The operator can already
  see how long a file is by looking.)

- **Per-candidate judgment.** For each candidate the agent reads the file and
  enough surrounding context to judge cohesion and placement, and returns a
  professional recommendation: whether to refactor, the nature of the problem
  (oversized, a low-cohesion "junk drawer", a single god-function, or a monolith
  better wrapped than split), and a concrete next step grounded in the project's
  own language and patterns (SOLID where it applies, or the appropriate
  alternative) — not a generic lecture and not snark.

- **Bounded, actionable advisory output.** The audit emits a short, ranked list
  of recommendations as a `Reported` outcome, each saying what is wrong, why it
  matters, and what to do. Not a dump of counts. A run that finds nothing worth
  refactoring resolves to an evidenced "no action recommended", not silence.

- **Refactors route to the issues lane, not specs.** When an operator acts on a
  recommendation via `send it`, the triage drafts an issue (`issues/<slug>/`, no
  spec delta) by default — a behavior-preserving refactor is exactly the issues
  lane's stated purpose. It drafts a spec change ONLY when the cleanup cannot be
  done without altering an observable contract (public API, wire format, CLI
  surface) or surfaces a genuine new capability decision. It never writes a spec
  merely to codify a metric, threshold, or "standard" — an issue has no `specs/`
  directory by contract, so there is structurally nowhere to reify a heuristic
  into law.

The pure-metric checks of `architecture_brightline` (function length, duplicate
signature, duplicate body) and the `.brightline-ignore` suppression file they
feed are removed. The `architecture_consultative` audit and its size-priority
refinement are removed. The cross-file rot that neither a per-file pass nor a
line count can see — parallel implementations, dead code, discoverability
failure — is explicitly out of scope here and recorded as a separate future
"deep-architecture" audit (`TODO.md`).

## Impact

- **Affected specs:** `orchestrator-cli` (remove the two audit requirements and
  the consultative size-priority refinement; modify the registered-audit list
  and the audit-triage write-scope to permit the issues lane; add the advisor,
  the issues-default routing, and a guard that no audit metric becomes a
  canonical requirement), plus mechanical name-purge in `chatops-manager` (the
  brightline/consultative top-line and emoji conventions, the stale-ignore
  clause) and `project-documentation` (the `.brightline-ignore` OPERATIONS.md
  requirement).
- **Affected code:** `audits/brightline.rs` + `audits/brightline/ignore.rs`
  (removed; the file-length scan survives, demoted to the advisor's selector),
  `audits/architecture_consultative.rs` (replaced by the advisor), the
  `AuditRegistry` registration, the audit-triage completion handler (permit an
  `issues/<slug>/` output subtree), the `audit-triage.md` prompt (issue-by-
  default routing; the no-metric-as-requirement guard), `config.example.yaml`,
  the `validate_audit_type_names` slug list, and the README audit table.
- **Peripheral canonical references to purge of the two slugs** (enumerated as
  tasks, not all reproduced as deltas in this draft): the cadence-schema
  scenario, the per-audit subprocess-timeout list, the install-wizard audit
  defaults, the LLM-driven validate list, the proposal-created notify list, the
  audit-substring-match scenarios, and the chatops top-line/emoji requirements.
- **Open decision — `.brightline-ignore`'s fate.** Removing the duplicate-
  signature metric leaves the ignore file with no consumer. A future deep-
  architecture audit doing semantic dedup may want an ignore mechanism, and a
  separate TODO already proposes extending the same file/`send it` pattern to
  the contradiction checks. This change removes `.brightline-ignore` as a
  brightline artifact; whether a successor ignore mechanism is reintroduced is
  deferred to that later work.
- **Out of scope:** the Level-3 deep-architecture audit (corpus-level rot);
  recorded in `TODO.md`.
