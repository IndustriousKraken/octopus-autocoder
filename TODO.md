# TODO

Design questions and future-work items that aren't yet ready for an OpenSpec change proposal. Each section is a candidate spec; when the design solidifies enough to draft, lift the section into `openspec/changes/<slug>/proposal.md`.

## Change-vs-canon contradiction pre-flight using RAG (future change)

`a19` ships the change-INTERNAL contradiction check. `a21` ships the RAG infrastructure. A future change would combine them to check changes against EXISTING canonical material — catching cases where a change's new ADDED requirements contradict canonical without explicitly modifying it.

The natural shape: for each ADDED requirement in the change, RAG-query the canonical corpus for top-K similar canonical requirements (excluding any the change explicitly MODIFIES or REMOVES). Hand the small bundle to an LLM with a "does this change's new requirement contradict any of these existing ones?" prompt. Findings flow through the existing `.needs-spec-revision.json` mechanism.

### Open design questions

1. **Scope of "v-canon"**: check the change's ADDED requirements against EVERY canonical capability, or scope to capabilities the change explicitly touches (the change's `specs/<cap>/spec.md` directory contents)? Narrower scope is cheaper AND lower false-positive rate; broader is more thorough. The retrieval step naturally narrows the search regardless of scope, but cost / noise tradeoffs differ.

2. **MODIFIED-as-resolution**: when the change MODIFIES a canonical requirement AND the new wording resolves what would otherwise be a contradiction, the check must recognize that and NOT flag. The detection needs to understand "this MODIFIED supersedes the canonical version we'd otherwise flag against." Probably implementable by: for each retrieved canonical chunk, check whether the change's MODIFIED block targets it by header. If yes, exclude from contradiction consideration (the change is updating it on purpose).

3. **LLM cost gating**: the check runs whenever a change has at least one ADDED requirement? Or only when a `canonical_rag.contradiction_check_enabled: true` flag is set (opt-in like `a19`'s internal check)? Cost per change is small (one LLM call, bounded input via retrieval) but non-zero.

4. **Interaction with RAG fail-open**: when `a21`'s store is unavailable (init failed, provider unreachable), the contradiction check has no canonical chunks to compare against. Fail-open (skip the check) OR fail-closed (block the change pending RAG availability)? Probably fail-open, matching `a14`'s posture.

5. **False-positive ergonomics**: the LLM may flag pairs that aren't really contradictions. The operator's recourse is `@<bot> clear-revision` without editing — same as `a19`. Worth noting that this check's higher abstraction makes false positives more likely than the structural checks; documentation should set expectations.

## Layer A RAG — full context injection (deferred)

`a21` exposes retrieval as a tool the implementer calls on demand. Layer A would proactively inject relevant canonical-spec chunks into the implementer's prompt at iteration start, before the implementer decides what to do. Larger surface area; bigger prompt budget impact; would need to interact with `a07`'s prompt-budget config.

Worth considering only if `a21`'s on-demand surface proves under-used (the implementer doesn't naturally call `query_canonical_specs` enough to surface relevant context). Hold for after `a21` ships and we see real usage patterns.

## install.sh + update.sh + docker-compose for Ollama as one-liner

The `a21` install wizard offers "install Ollama via docker" as option 1 but stops short of auto-running `docker compose up`. A more aggressive quick-start would: detect docker; offer to run the compose file as part of the install wizard; wait for Ollama to come up; pull the embedding model; verify the embed pipeline end-to-end. Trade-off: more wizard surface area, more failure modes. Worth doing only if operators report friction with the manual `docker compose up` step.

## Brightline-ignore extension to RAG-aware contradiction check

`a15` adds `.brightline-ignore` for intentional code duplication. The same concept could apply to the contradiction-pre-flight check above: a `.contradiction-ignore.yaml` lists requirement pairs the operator has reviewed AND confirmed are not actually contradictory. The check honors entries and stops flagging known-good pairs. Same architecture (file at workspace root, LLM-populated via `send it`, audit-time stale-pruning).

Defer until the contradiction-pre-flight ships and we see false-positive rates.

## Model attribution on reviewer / executor / audit comments

Operator-facing comments (code review, executor implementation notes, audit findings, contradiction-check findings) don't currently identify which model produced them. With multiple LLM providers/models configurable across these surfaces — AND with operators experimenting across reviewer tiers — the lack of attribution makes it hard to associate a comment's quality with the model that produced it.

The fix is small. A redaction-safe accessor on the resolved config (the same primitive that gives selective config access without leaking API keys) returns `(provider, model)` for each LLM-driven surface. Each comment composer prepends or appends a one-line attribution: `*Reviewer: openrouter/moonshotai/kimi-latest*` (or `*Executor: ...*`, `*Auditor: ...*`). Render points: `revisions.rs:~1250` (rerun reviews), the initial-review PR-body builder, the executor implementation-notes section, each audit's chatops + PR comment formatter.

Scope considerations: identifier format should be stable across providers (e.g. `<provider>/<model>` rather than each provider's native naming). The accessor MUST refuse to return anything that could be an API key, base URL, or other secret-bearing field — explicit allowlist of safe fields, not a denylist.

Worth doing soon — it cleanly closes the "which model produced this?" gap that operators (and Claude itself, helping operators debug) currently bridge by memory.

## Auto-revise trigger trace + critical-evaluation prompt for the revising agent

Two related concerns about the auto-revise pipeline that surfaced during the multi-reviewer PR #79 trial:

**(1) Trigger-path trace.** Confirm which combination of `Verdict` (Approve | Block) + `should_request_revision: true` actually fires auto-revise, AND whether the operator-triggered rerun path (`@<bot> code-review`) participates or only the initial-review path does. Observed: owl-alpha's rerun review had two `should_request_revision: true` items (one impossible to action, one targeting a fabricated test name) — neither triggered an auto-revise commit. That's accidental safety; the underlying logic may or may not currently gate against this case correctly.

**(2) Critical-evaluation prompt.** When auto-revise DOES fire, the prompt handed off to the revising agent should explicitly instruct it to evaluate the original reviewer's request critically — not assume the previous reviewer is correct about the need. Concrete reviewers tested (owl-alpha, MiMo) have both produced `should_request_revision: true` items that would actively damage the codebase if applied: removing a spec-traced test the reviewer mistakenly believed was redundant; churning working idiomatic code (`.tmp` extension → `NamedTempFile`) for protection that doesn't apply. The implementing agent should: (a) read the actual code at the cited location; (b) verify the reviewer's claim against current state; (c) reject the revision when the claim is wrong; (d) post a chatops comment naming what it rejected and why, so the operator sees the trail. Models with strong instruction-following (Claude, Opus) will do this naturally if asked; cheaper executors may need the rejection mechanic spelled out explicitly.

Worth scoping together because (2)'s prompt is only load-bearing if (1) confirms the trigger fires on these inputs.

## Reviewer `mode: per_change` not honored on rerun path

When `reviewer.mode: per_change` is set in config, the expected output is one `## Code Review: <slug>` section per change. Observed on PR #79 reruns (owl-alpha, laguna-m.1): both produced a single bundled `## Code Review` block, suggesting the rerun path forces `ReviewerMode::Bundled` regardless of config — OR the config-to-reviewer mode threading uses the default rather than reading from `ReviewerConfig`. Infrastructure exists (`PerChangeSection`, `with_mode(ReviewerMode::PerChange)`, the `per_change_sections: Vec<...>` field in `ReviewReport`); investigation needs to trace the operator-trigger code path (`@<bot> code-review` → `review_pr_at_state` in `revisions.rs`) and confirm where the mode is or isn't propagated.

Worth fixing because per-change review is materially more useful than bundled when a PR carries multiple unrelated changes — operators want to see "change a35 is approved; change a36 has concerns" not one combined verdict that hides per-change differences.

## On-demand audit re-run after operator merges a fix

When an audit fires (drift, brightline, etc.) and the operator addresses the findings via `send it`, the audit's `last_run_sha` is unchanged — the audit only re-runs when HEAD changes. The next audit fire could be days later. An operator who fixes findings and wants to verify the fix worked has to wait for the next cadence OR explicitly re-trigger via `@<bot> audit <type> <repo>`.

Could be improved: when `send it` produces a PR that merges, automatically re-queue the audit that triggered the `send it`. Closes the loop without operator action. Small spec.

## Prompt-content tests are the wrong category — delete the prose assertions, keep the plumbing

`low_confidence_finding_filtering_explicit_in_prompt` (`autocoder/src/audits/security_bug.rs:335`) asserts FOUR verbatim full sentences are present in the embedded security-bug-audit prompt. The 2026-06-02 prompt rewrite reworded two of them without changing their meaning — "Only emit a change for findings you are highly confident about" → "Emit only findings you are highly confident about", AND it dropped the literal "When in doubt, DON'T emit" (the concept survives as "If any is missing, drop the finding"). Two assertions now fail; `cargo test` is red. (Secondary process smell: the PR #81 implementer agent mis-attributed this failure as "pre-existing and unrelated" and shipped anyway, never connecting a prompt-content test failure to the prompt edit on the same branch.)

The deeper problem is not brittleness — it's that this is the wrong kind of test. It is a change-detector that pins the prompt's prose to itself: it passes because someone typed the words and fails because someone retyped them differently. It encodes no independent truth, cannot distinguish a better reword from a deletion, and does not test the actual feature (whether the audit drops low-confidence findings — that is model behavior, invisible to a substring check). The author's rationale ("break CI rather than the operator's mailbox") assumes prompt edits go unreviewed, but they ride the same PR/review pipeline as code, so review already covers it.

Distinguish two categories in the prompt/audit test suites:

- **Plumbing tests (KEEP / strengthen)** — these test logic that can break silently: placeholder substitution (`{{change_body}}`), `MAX_PROPOSALS:` injection, prompt-file override/precedence in the loader, the non-empty-prompt guard, sandbox tool-list construction, custom-prompt-sentinel resolution. The existing `MAX_PROPOSALS:` and drift custom-sentinel tests are this kind; leave them.
- **Prose-content tests (DELETE)** — any `prompt.contains("<sentence>")` assertion on instruction wording. They add CI churn and catch nothing review doesn't.

Fix:
1. Delete `low_confidence_finding_filtering_explicit_in_prompt` (and any sibling prose-assertion tests the sweep finds). Do NOT replace it with "robust short-token" versions — converting four brittle sentence-checks into four slightly-less-brittle token-checks just makes a bad test marginally less bad; the category is the problem.
2. Sweep `autocoder/src/audits/*.rs`, `code_reviewer.rs`, and the loader tests for the same prose-assertion pattern; delete each, preserving only plumbing/structural/sentinel assertions.
3. OPTIONAL narrow exception: prompts can be edited by agents (documentation_audit, future self-modifying paths) where "reviewed prose" is weaker. If agent-edit insurance is wanted, leave ONE coarse anti-deletion tripwire per agent-editable prompt — "still mentions `confidence` at all" — explicitly documented as a tripwire, not a content test. The project's self-modification guardrails already cover most of this, so this is optional.

Near-term: the red test weakly blocks the `cargo test` acceptance gate for every queued change until removed (agents are working around it by declaring it unrelated). Deleting it is a one-line cleanup worth doing before pushing the queued batch.

---

# Agentic fleet migration (planned spec stream)

A coordinated stream of changes that gives EVERY LLM-driven step — executor, reviewer, pre-checks, post-checks, audits — the same shape: a wrapped agent CLI running an agentic session in a read-only-capable sandbox, with structured output via per-role MCP tools, and a swappable CLI strategy so any provider's model (Anthropic, OpenRouter, Ollama) can drive any role. The purpose is to make larger/more complex LLM-built projects possible by keeping them on the rails with diverse, independent, well-controlled checks — model diversity is load-bearing (a different model reviewing than implementing catches blind-spots a single model's training assumptions miss).

Author the changes in dependency order. Each entry below is a candidate spec; lift it into `openspec/changes/<slug>/` when its turn comes. Keep this manifest in sync as changes graduate.

## Architecture umbrella (shared across the stream)

**The agentic-run primitive.** Wrap a CLI as a subprocess; hand it a prompt; let it run its own session to completion (it decides when done; the CLI owns its own context compaction — the executor already proves long multi-step sessions work). Shared sandbox tools for every role: `Read`, `Grep`, `Glob`, AND `query_canonical_specs` (the a21 semantic-search MCP tool, now fleet-wide). Structured output via a per-role `submit_*` MCP tool — NO stdout-JSON parsing anywhere (stdout-JSON is the fragility behind the Grok-refuses / Qwen-9B-confabulates behavior).

**CLI strategy (two jobs, one trait).** Each wrapped CLI has a strategy implementation that (1) builds the invocation — flags, sandbox/allowed-tools, MCP-config-file format — AND (2) translates the resolved model config into that CLI's model-selection mechanism. `claude` → `ANTHROPIC_BASE_URL` + `ANTHROPIC_AUTH_TOKEN` + `ANTHROPIC_MODEL` env, AND only speaks the Anthropic Messages wire format. `opencode` → its provider config file, speaks OpenAI-compatible + Ollama + many others natively. The Anthropic-wire constraint is exactly why a provider-agnostic CLI is required for non-Anthropic agentic runs — it is not optional given model diversity is the point.

**Model registry.** A top-level `models:` block defines `(provider, model, base_url, key)` once under a nickname; every role references a nickname (`model: beefy_security`) instead of duplicating provider/key. The registry entry's `provider` resolves the default CLI strategy for agentic runs (anthropic → claude; openai_compatible / ollama → opencode; overridable per model since opencode can also drive Anthropic). The operator thinks only in models; the CLI is resolved underneath. This removes any per-role `command:` field.

**`kind: agentic | oneshot`.** Agentic is the default for reasoning roles. `oneshot` (HTTP `LlmClient.complete()`, the a37 surface) is retained as (a) a fast/cheap opt-in, AND (b) the only path for non-Anthropic models during the claude-only window before the opencode strategy lands. RAG embedding stays one-shot PERMANENTLY — producing an embedding vector is a single forward pass, not a reasoning session; `agentic` is meaningless for it.

**Per-role submit tools.** `submit_review`, `submit_findings`, `submit_contradictions`, `submit_verdict` — per-role, not one generic tool (matches the executor's existing `outcome_*` family idiom; clearer schemas, model can't pick the wrong one). Each schema mirrors the structure that role's downstream consumer needs (e.g. `submit_review` carries verdict + per-concern entries with `actionable_request` + `should_request_revision`, per change, to preserve auto-revise + per_change).

## Change sequence

### 1a. Auto-revise trigger fix (a46 — authored; independent, near-term, live dormant bug)
`partition_and_annotate_reviewer_revisions` (polling_loop.rs:5109) returns empty unless `verdict == Block`, so auto-revise never fires for the common `Concerns` verdict — even when concerns carry `should_request_revision: true` + a valid `actionable_request`. Conservative reviewers rarely Block, so the feature is dormant. Fix: trigger on the actionable signal (`should_request_revision: true` + non-empty `actionable_request`) REGARDLESS of verdict; rename `reviewer.auto_revise_on_block` → `reviewer.auto_revise` (serde alias). Bounded by the EXISTING `executor.max_revisions_per_pr` cap (which caps all revisions) until 1b refines it, so no runaway in the gap. Authored as `a46-auto-revise-fires-on-actionable`.

### 1b. Caps reframing — automatic-only (a47 — sibling of a46)
Reframe caps so only AUTOMATIC chains are bounded: the revision cap counts ONLY reviewer-marked (`<!-- reviewer-revision -->`) revisions; human `@<bot> revise` is uncapped. Rename `executor.max_revisions_per_pr` → `executor.max_auto_revisions_per_pr` (alias). Uncap human `@<bot> code-review` entirely (all re-reviews are human — the "No reviewer re-run after revision lands" requirement guarantees no automatic re-review — so the re-review cap guards a deliberate human act with no runaway risk; default it to unlimited, keep the field as an opt-in ceiling). Touches ~5 reviewer-spec requirements + the orchestrator-cli revision-cap requirement; that breadth is why it's split from a46. Compounding fix to fold in here OR separately: malformed-verdict-defaults-to-Approve on the rerun route (memory `reviewer-verdict-parse-failure-defaults-to-approve`).

### 2. Model registry (`models:` block + nickname references)
Top-level `models:` registry; migrate every LLM-consuming block (reviewer, contradiction-check, audits' future agentic config, RAG) to reference nicknames. Backward-compat: keep inline per-block config accepted, with the registry as the deduped form. Foundational — lands before the agentic roles proliferate so they reference nicknames from birth. Defines the `provider → default CLI` resolution rule.

### 3. Extract the agentic-run primitive + CLI-strategy trait (claude impl) + `submit_*` MCP family
Pull the shared primitive out of the executor + audit `run_subprocess` paths (refactor both onto it, NO behavior change). Define the `CliStrategy` trait with the claude implementation. Add the per-role `submit_*` MCP tool family to the MCP server. This is the architectural spine; everything below hangs off it. Rule-of-three justifies extracting now (executor + audits + incoming reviewer).

### 4. Migrate audits stdout-JSON → `submit_findings` MCP
Move drift, architecture_consultative, documentation_audit, AND the specs-writing audits (missing_tests, security_bug) off fragile stdout-JSON onto `submit_findings` / `submit_*`. Kills the confabulation-on-weak-models risk across the fleet. Depends on 3.

### 5. Agentic reviewer (`kind: agentic` default, claude-only)
Reviewer on the primitive: read-only sandbox, reads files on demand (no 2M-char truncation), `submit_review` structured output. Preserve the full downstream contract: `reviewer.mode: per_change`, auto-revise (post-#1 semantics), the `@<bot> code-review` re-review verb. Retain `kind: oneshot` as fallback. Depends on 3 (and ideally 1 + 4).

### 6. Agentic contradiction-check (second one-shot holdout → primitive)
Move `change_internal_contradiction_check` off `LlmClient.complete()` onto the primitive with `submit_contradictions`. Depends on 3.

### 7. OpenCode CLI strategy (provider-agnostic)
Second `CliStrategy` implementation: opencode. Unlocks agentic + OpenRouter/Qwen/Ollama + context for every role. The model-diversity enabler — without it, agentic runs are Anthropic-only, which defeats the cross-check purpose. Conformant MCP-config generation for opencode is the main work (each CLI gets its own config-file format). Depends on 3; sorts late but is in-stream, not deferred.

### 8. Verifier trio on the primitive (may split into multiple changes)
Three boundary gates, same shape, each a `submit_verdict` role: (a) change-internal consistency [in] — supersedes/absorbs the contradiction-check; (b) change-vs-canon contradiction [in] — the RAG-pre-flight already sketched earlier in this file; (c) did-the-code-implement-the-spec [out] — the aspirational verifier from the README roadmap. Well-bounded gates can run super-budget models (Ollama via opencode). Depends on 3 + 7 (for budget models). Keep mentally distinct from the reviewer (code quality) — different lens, different verdict semantics.

## Default prompts assume Rust/this-project tooling — make them language-neutral (near-term, independent)

The embedded default prompts leak Rust- and this-project-specific tooling into prompts that run against ANY managed repo. Observed: an agent reviewing a Python project noted "there's no Clippy in Python" — the implementer/revision prompts instruct `cargo clippy --all-targets -- -D warnings` and `cargo test` regardless of the target project's language. (Self-inflicted: the 2026-06-02 prompt rewrite baked `cargo clippy` into `prompts/implementer.md`'s final_answer worked example AND into a45's revision-prompt content guidance — both need cleanup as part of this.)

`openspec validate --strict` is fine to keep (every managed repo uses OpenSpec). The leak is language-specific build/lint/test/format commands. Fix options:
- **Detect-and-run (simplest):** prompts say "run the project's linter / formatter / test suite (detect from the repo's build config — Cargo.toml, package.json, pyproject.toml, go.mod, etc.)" instead of naming `cargo` commands. Language-neutral; relies on the agent to detect.
- **Per-repo tooling config:** a config block names the lint/test/format commands per repository, injected into prompts via placeholders. More precise, more config surface.
- **Mix:** default to detect-and-run; allow a per-repo override block.

Scope when authored: sweep ALL default prompts under `prompts/` for language/environment assumptions (not just implementer/revision — check audit + triage + reviewer prompts too). Pairs naturally with the model-registry work (per-repo config surface already in motion) but is independent and near-term because it's actively producing wrong PR comments.

## Open design questions to resolve while authoring

- **Submit-tool relay vs in-process.** The executor's outcome tools relay to the daemon via the control socket. Reviewer/audit submit tools — same relay, or a lighter in-process capture? The relay buys uniformity AND daemon-side ownership of results.
- **Per-role sandbox toolsets.** All get Read/Grep/Glob + query_canonical_specs. Does the reviewer get `Bash` (to run a build/test) or stay strictly read-only? The audits get Bash today; the reviewer historically did not.
- **Registry migration ergonomics.** Hard cutover to nicknames, or indefinite dual-acceptance of inline + registry? Affects how aggressively change 2 rewrites existing configs.
- **opencode MCP maturity.** Spike: confirm opencode's MCP-tool support is robust enough to carry a validated `submit_*` before committing change 7's design (knowledge-cutoff caveat — verify against the current opencode release).
