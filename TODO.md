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

## Model attribution on reviewer / audit comments — AUTHORED as `a49-model-attribution-on-llm-output`

A redaction-safe accessor (positive allowlist: provider + model; never api_key/base_url) returns `<provider>/<model>` (provider is the `LlmProvider` KIND, e.g. `openai_compatible/moonshotai/kimi-latest`, not the upstream brand). Each daemon-composed operator-facing output carries `*<Role>: <provider>/<model>*` — `Reviewer` (initial + rerun), `Auditor (<type>)`, `Contradiction-check`. The accessor's redaction is testable behavior (not a content test). The **executor** implementation-notes are explicitly OUT of scope: the executor wraps the Claude CLI with no daemon-known model; its attribution defers to the model-registry work (fleet change 2).

## Auto-revise trigger trace + critical-evaluation prompt — BOTH RESOLVED

**(1) Trigger-path trace** → resolved by `a46-auto-revise-fires-on-actionable`: the trigger gated on `verdict == Block` (polling_loop.rs:5109), so it never fired for the common `Concerns` verdict; a46 moves the trigger to the per-concern `should_request_revision` + `actionable_request` signal regardless of verdict.

**(2) Critical-evaluation prompt** → authored as `a52-revision-agent-critical-evaluation`: the revision prompt instructs the agent to read the cited code, verify the request's claim, decline/partially-honor when wrong, and report the declination via `final_answer` (surfaced by a45). a52 also fixes the latent bug where a no-change declination false-reports as a commit/push failure — clean-tree `Completed` becomes a reported declination. Stacks on a45. Prompt guidance is drift-audited intent, not a content test (see [[test_behavior_not_message_content]]).

## Reviewer `mode: per_change` not honored on rerun path — AUTHORED as `a53-reviewer-mode-honored-on-rerun`

Spec-compliance bug: canon already requires the rerun path to honor `reviewer.mode` (the `Reviewer entry point is reusable…` requirement says the function "SHALL use the configured `reviewer.mode`"; the re-review requirement expects per-change output). Root cause: `review_pr_at_state_with` (code_reviewer.rs:418) calls the bundled entry point unconditionally, AND the `ReviewResult` contract omitted `per_change_sections` so the rerun composer (revisions.rs:1281) had nothing to render. a53 adds `per_change_sections` to the contract, dispatches per mode in the reusable function, and renders per-change sections in the rerun comment.

## Prompt-content tests are the wrong category — AUTHORED as `a48-tests-assert-behavior-not-prompt-content`

A test must assert what the code DOES (behavior) or that derived output matches its source (derivation); it must never read a real shipped prompt/message and assert a hand-authored substring of its prose. Coarse "tripwire" content checks (assert a URL/keyword is present) are the same anti-pattern, not an exception — that includes the `a41` OpenSpec-pointer regression test. Prompt design *intent* lives in requirement prose and is verified by the drift audit's semantic judgment, not a unit test.

a48 encodes this as a project-documentation requirement (`Tests assert behavior or derivation, never message wording` — the source of truth the drift audit enforces against), removes the `a41` requirement, softens the orchestrator-cli `Security & bug audit` confidence scenario and the code-reviewer scope scenario from verbatim → intent + sentinel-substitution, and deletes the offending tests (incl. the red `low_confidence_finding_filtering_explicit_in_prompt`). The broader sweep of other audits' wording scenarios is intentionally left to the drift audit (no hand-sweep). See [[test_behavior_not_message_content]].

## On-demand audit re-run after operator merges a fix — DROPPED (resolved by design)

Not needed. Post-`a43` (auditors send spec-only PRs), a merged spec PR triggers the executor to produce the code change, which advances HEAD; the audit's `requires_head_change` gate then clears naturally on the next cadence. The original concern (`last_run_sha` unchanged) only held under the old one-PR-with-code flow. Operators wanting immediate verification still have `@<bot> audit <type> <repo>`.

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
Reframe caps so only AUTOMATIC chains are bounded: the revision cap counts ONLY reviewer-marked (`<!-- reviewer-revision -->`) revisions; human `@<bot> revise` is uncapped. Rename `executor.max_revisions_per_pr` → `executor.max_auto_revisions_per_pr` (alias). Uncap human `@<bot> code-review` entirely (all re-reviews are human — the "No reviewer re-run after revision lands" requirement guarantees no automatic re-review — so the re-review cap guards a deliberate human act with no runaway risk; default it to unlimited, keep the field as an opt-in ceiling). Touches ~5 reviewer-spec requirements + the orchestrator-cli revision-cap requirement; that breadth is why it's split from a46. The malformed-verdict-defaults-to-Approve fix (memory `reviewer-verdict-parse-failure-defaults-to-approve`) is NOT folded into a47: it resolves for free at change 5 (agentic reviewer) — once the reviewer submits via an MCP `submit_review` tool, a malformed verdict becomes a bad tool call, which that change specs to discard the review and alert the operator. No separate pre-fix needed.

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

## Default prompts assume Rust/this-project tooling — AUTHORED as `a51-language-neutral-default-prompts`

Detect-and-run approach (the TODO's preferred "mix" default): a project-documentation requirement states default prompts name the project's own tooling (detected from the build config), not a specific toolchain; `openspec validate --strict` stays (shared). Drift-audited, no content test (a negative "no `cargo`" scanner is unenumerable AND the wording-assertion anti-pattern). Sweeps `prompts/`; concrete fixes in `implementer.md` (`cargo clippy` ×2) and `brownfield-draft.md` (`cargo test`). Per-repo tooling-config override deferred to a future change. SHOULD land after a45 to also clean a45's worked example.

## Open design questions to resolve while authoring

- **Submit-tool relay vs in-process.** The executor's outcome tools relay to the daemon via the control socket. Reviewer/audit submit tools — same relay, or a lighter in-process capture? The relay buys uniformity AND daemon-side ownership of results.
- **Per-role sandbox toolsets.** All get Read/Grep/Glob + query_canonical_specs. Does the reviewer get `Bash` (to run a build/test) or stay strictly read-only? The audits get Bash today; the reviewer historically did not.
- **Registry migration ergonomics.** Hard cutover to nicknames, or indefinite dual-acceptance of inline + registry? Affects how aggressively change 2 rewrites existing configs.
- **opencode MCP maturity.** Spike: confirm opencode's MCP-tool support is robust enough to carry a validated `submit_*` before committing change 7's design (knowledge-cutoff caveat — verify against the current opencode release).
