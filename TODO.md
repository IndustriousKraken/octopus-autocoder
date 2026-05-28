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

## On-demand audit re-run after operator merges a fix

When an audit fires (drift, brightline, etc.) and the operator addresses the findings via `send it`, the audit's `last_run_sha` is unchanged — the audit only re-runs when HEAD changes. The next audit fire could be days later. An operator who fixes findings and wants to verify the fix worked has to wait for the next cadence OR explicitly re-trigger via `@<bot> audit <type> <repo>`.

Could be improved: when `send it` produces a PR that merges, automatically re-queue the audit that triggered the `send it`. Closes the loop without operator action. Small spec.
