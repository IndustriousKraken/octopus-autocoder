# Global rules and the `[rules]` gate

The **global rule corpus** is a collection of portable, project-agnostic
engineering rules — hard-won lessons the operator wants every project autocoder
works to honor (no futile tautological tests, prefer composition over deep
inheritance, no committed secrets, and so on). The **`[rules]` gate** of the
verifier framework checks each OpenSpec change against that corpus before the
executor runs, and holds any change whose spec deltas would violate a rule.

`[rules]` is the corpus-parameterized sibling of the `[canon]` gate: the same
read-only agentic machinery, but the comparison corpus is the global rule corpus
(instead of this project's canonical specs), and each finding names the violated
rule by its stable id (instead of a canonical requirement). See
[OPERATIONS.md](OPERATIONS.md) for the verifier framework as a whole.

## The rule protocol: minimal prose, not contract language

A rule is interpreted by an LLM's **judgment**, not by an algorithm. The format
therefore carries no machinery that exists only to make text machine-parsable.
Each rule is:

- a one-sentence **`rule`** — the assertion the gate checks;
- an optional **`intent`** — a short rationale/exceptions paragraph that informs
  the judgment (and feeds future retrieval); and
- a **stable id** — so a violation can name the rule it broke.

Deliberately **NOT** OpenSpec contract language: there are **no**
`SHALL`/`MODIFY`/`ADD`/`REMOVE`/`RENAME` deltas, **no** scenarios, **no** task
lists, and **no** archive/compose step. Those serve OpenSpec's *edit-the-canon*
lifecycle, which rules do not have — contract keywords would add authoring
friction without adding checkability, since the gate's model judges the prose
directly.

Rules are **edited directly**; git history is the change record. There is no
delta block, no archive step, and no canon-compose step for rules.

### Authoring a rule

A rule is a single Markdown (`.md`) or text (`.txt`) file. Its **stable id is
the file path relative to the corpus root, without the extension** — e.g.
`no-secrets.md` → `no-secrets`, and `testing/no-tautological-tests.md` →
`testing/no-tautological-tests`. The file body is the one-sentence rule,
optionally followed by an `intent` paragraph. Example:

```markdown
Secrets are never committed to the repository — they live in environment
variables or a secrets manager, never in tracked files.

intent: A committed secret is exposed to everyone with repo read access and
lives forever in git history. The rule covers API keys, tokens, passwords, and
private keys, whether in source, config, fixtures, or test data.
```

Keep each rule to one assertion. Split unrelated assertions into separate files
so a violation can point at exactly one rule.

## Corpus layout: flat or grouped

The corpus is a directory of rule files. It MAY be **flat**:

```
rules/
  no-secrets.md
  no-tautological-tests.md
  prefer-composition.md
```

…or **grouped into registers** (one level of subdirectories) for related rules:

```
rules/
  security/
    no-secrets.md
    no-eval-of-input.md
  testing/
    no-tautological-tests.md
  design/
    prefer-composition.md
```

A `README.md` at any level is ignored (it documents the corpus, it is not a
rule), as are hidden files. The grouped layout's register name becomes part of
the rule id (`security/no-secrets`).

### Scale: feed-all now, retrieval later

While the corpus is small, the gate feeds **all** rules to its session (inlined
into the prompt). The only structural pressure the protocol anticipates is
**retrieval at scale**: when the corpus outgrows the context window, a relevant
subset is selected (coarse by register, then semantic) — the rule shape does not
change. In the code this is the seam at
`preflight::global_rules::load_rule_corpus`: it returns all rules today; a
relevant-subset selector replaces the "load all" body without changing the
`Rule` shape or the call site. The protocol never grows toward machine-instruction
formality — judgment is irreducible.

## Pointing autocoder at the corpus

The corpus location is configured under `executor.global_rules.corpus`. It is
either a **local directory path** OR a **git repo URL** the daemon clones at
startup (and reuses on subsequent restarts, under the daemon cache dir).

```yaml
executor:
  global_rules_check: enabled        # opt-in (disabled by default)
  global_rules_check_llm:            # required when enabled
    provider: anthropic
    model: claude-haiku-4-5-20251001 # cheap; the prompt is small
  global_rules:
    corpus: /etc/autocoder/global-rules            # a local path …
    # corpus: https://github.com/acme/eng-rules.git  # … OR a git repo URL
```

A **local** corpus path may begin with `~/` or `$HOME/` (or be a bare `~` /
`$HOME`); autocoder expands the leading token to the operator's home directory
before resolving the path — so `corpus: ~/.config/autocoder/global-rules` (the
value the check-only installer's `install-verify.sh` writes) resolves to
`<home>/.config/autocoder/global-rules`. Expansion is leading-only: a tilde
elsewhere in the value and the `~user` form are left untouched. A **git-URL**
corpus is used as-is — it is never subject to home expansion.

Enabling `global_rules_check` without **both** a configured
`global_rules_check_llm` **and** a resolvable `global_rules.corpus` fails the
daemon at startup (fail-fast), exactly as the `[canon]` gate does for a missing
model. A local-path corpus must exist as a directory; a git-repo corpus must
clone successfully.

## How the gate runs: locally and server-side

Because `[rules]` is a verifier gate, it runs in both places that matter:

- **Server-side, pre-executor** — the enforcement guarantee that no change in
  any repo violates the global rules. This path is normative: the gate's
  behavior, opt-in, and fail-closed posture are fully defined here and do not
  depend on any local runner.
- **Locally via `verify`** — the local runner runs "the enabled spec-checking
  gates" generically, so it picks up `[rules]` with no extra wiring; the
  spec-box config carries the model block and corpus location. Local feedback is
  an accelerator; the server is the guarantee.

On a violation the gate writes a `.needs-spec-revision.json` marker whose
`revision_suggestion` names each violated rule by id, posts the
`SpecNeedsRevision` chatops alert, and halts the queue walk — the executor is
not invoked. The gate **fails closed**: a session error, an unregistered
strategy, a never-corrected submission, or a session with no submission holds
the change with a structured `gate_error` marker and a distinct "gate FAILED TO
RUN — change held" alert, rather than waving the change through as "no
violations". An empty submission is a clean pass. The `[rules]` verdict is
rendered in the PR's gate-ledger section alongside the other gates.
