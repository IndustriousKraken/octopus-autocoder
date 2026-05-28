## Why

`a17`'s mechanical pre-flight catches structural defects (MODIFIED title missing from canonical, ADDED title already present, etc.). It doesn't catch semantic defects — a change whose requirements are individually well-formed AND archivable but contradict each other.

Examples of self-contradictions a mechanical pre-flight cannot catch:

- ADDED requirement A: `All secrets SHALL live in environment variables`. ADDED requirement B: `The API key SHALL be stored in config.yaml under api_key`. Both archive cleanly. They directly contradict.
- ADDED requirement A: `No operation SHALL exceed 60 seconds`. ADDED requirement B: `The full rebuild workflow SHALL complete its scan, validation, and PR-open steps end-to-end`. The rebuild routinely takes 5+ minutes; A and B can't both hold.
- ADDED requirement A: `The audit framework SHALL be opt-in by default`. MODIFIED requirement B (which DID match canonical): canonical requirement gets a new scenario requiring "Every registered audit runs on each polling iteration." MODIFIED B says "always runs" while ADDED A says "opt-in by default" — they describe incompatible semantics.

The shared pattern: a future implementer (human or LLM) reading the post-archive canonical spec would have to choose which requirement to honor, because honoring both is impossible. Catching this BEFORE the implementer runs avoids the same expensive-then-fail cycle `a17` addresses for the structural case.

The check is an LLM call. Pure-text logic can't reliably detect semantic contradiction; specs use domain language AND the contradiction often hides in the interplay of multiple SHALL clauses. The LLM input is small (the change's spec deltas + the canonical specs for any MODIFIED capabilities) so the cost is low.

This is scoped to **change-internal** contradiction: the deltas within the same change fighting each other. Change-vs-canonical contradiction (e.g. the change ADDs a requirement that fights an existing canonical requirement the change doesn't MODIFY) is a separate, larger problem AND is deferred to a follow-up spec. The internal case is the cheaper AND more frequent win — operators draft self-contradicting specs more often than they accidentally fight unrelated canonical material.

## What Changes

**New pre-flight step `change_internal_contradiction_check` AFTER `a17`'s archivability check AND BEFORE the executor invocation.** When the change has at least one delta (any kind), the daemon SHALL invoke a configurable LLM with a `prompts/change-contradiction-check.md` prompt template + the change's spec-delta files concatenated. The LLM returns JSON conforming to:

```json
{
  "contradictions": [
    {
      "requirement_a": "<title or 'inline scenario' anchor>",
      "requirement_b": "<title or 'inline scenario' anchor>",
      "summary": "<one-sentence description of the conflict>"
    }
  ]
}
```

An empty `contradictions` array means the LLM found nothing. Any non-empty result triggers the same `.needs-spec-revision.json` marker mechanism `a17` uses, with the JSON contents embedded in the marker's `revision_suggestion`.

**The check is opt-in initially.** A new config field `executor.change_internal_contradiction_check` (`enum { disabled, enabled }`, default `disabled`) gates the LLM call. Operators must explicitly enable it. Reasons for the conservative default:

- The LLM cost (small, ~$0.01 per check at current pricing) is non-zero AND charged per-change regardless of whether the check finds anything.
- Early operator experience may produce false positives (an LLM flagging things that aren't really contradictions); the gated rollout lets operators opt in when ready.
- The check's prompt template needs iteration AND operators may want to override it per their domain.

**Embedded prompt template, overridable.** Default `prompts/change-contradiction-check.md` ships in the binary via `include_str!`. Overridable via `executor.change_internal_contradiction_check_prompt_path` config (parallel to other prompt-override fields). The default prompt:

> You are checking a single OpenSpec change for internal contradictions: requirements within this change that cannot all hold simultaneously. Input: the change's spec-delta files concatenated (ADDED + MODIFIED + REMOVED + RENAMED blocks across every capability the change touches).
>
> A contradiction is when honoring requirement A would prevent honoring requirement B. Examples:
> - A says "all secrets in env vars"; B says "the API key in config.yaml" (same change adds both)
> - A caps an operation at N seconds; B describes a workflow that exceeds N seconds
> - A enforces a default ("audits opt-in"); B's MODIFIED scenario contradicts the default ("audits always run")
>
> NOT a contradiction:
> - A says "feature X exists"; B says "feature Y exists" where X AND Y are different AND compatible
> - Wording differences with no semantic conflict
> - Different scenarios under the same requirement covering different cases (e.g. "happy path" + "error path") are not contradictory by virtue of being different
>
> Read every delta block. Apply domain knowledge — a "5-minute workflow" IS longer than a "60-second cap" even if the math isn't spelled out; MongoDB IS NoSQL even if neither requirement says "NoSQL."
>
> Output exactly ONE JSON object to stdout:
> ```json
> { "contradictions": [{ "requirement_a": "...", "requirement_b": "...", "summary": "..." }] }
> ```
> No commentary outside the JSON. Empty array if no contradictions.

**Failure handling.** The pre-flight LLM call MAY fail (network, rate-limit, malformed response). On failure, autocoder logs a WARN, treats the check as "no contradictions detected" (fail-open — same conservative bias as `a14`'s transient-failure handling), AND proceeds to the executor. The operator sees the WARN in journalctl AND can investigate; the LLM failure does NOT block the daemon.

**Configuration:**

```yaml
executor:
  change_internal_contradiction_check: disabled  # | enabled. Default disabled.
  change_internal_contradiction_check_prompt_path: null  # path to override file; null → embedded default.
  change_internal_contradiction_check_llm:
    provider: anthropic  # or openai_compatible — same surface as reviewer config
    model: claude-haiku-4-5-20251001  # cheap; the prompt is small
    api_key_env: ANTHROPIC_API_KEY
```

The LLM config is its own block separate from `reviewer:` because: (a) operators may want a cheaper model for the check than for full reviews; (b) the prompt + input are very different shapes; (c) failures should be isolated from the reviewer's behavior.

## Impact

- **Affected specs:**
  - `orchestrator-cli` — one ADDED requirement: `Change-internal contradiction pre-flight check (opt-in)`. Defines the check, the gate, the prompt template, the marker re-use from `a17`, AND the fail-open posture.
  - `project-documentation` — one ADDED requirement: `CONFIG.md and OPERATIONS.md document the contradiction-check fields and the LLM-call cost model`.
- **Affected code:**
  - `autocoder/src/preflight/change_contradiction.rs` (new) — module containing the check:
    ```rust
    pub async fn check_change_internal_contradictions(
        workspace_root: &Path,
        change_slug: &str,
        llm: &dyn LlmClient,
        prompt_template: &str,
    ) -> Result<Vec<ContradictionFinding>>;
    pub struct ContradictionFinding {
        pub requirement_a: String,
        pub requirement_b: String,
        pub summary: String,
    }
    ```
  - `autocoder/src/polling_loop.rs` — pre-executor pipeline:
    1. `a17`'s archivability check (mechanical).
    2. If `executor.change_internal_contradiction_check: enabled`: `a19`'s contradiction check (LLM).
    3. Then `executor.run(...)`.
  - `autocoder/src/state/needs_spec_revision.rs` — extend the schema's `revision_suggestion` field (already free-form text per existing spec) to carry the contradictions list when this check is what triggered the marker. The dedicated `unarchivable_deltas` field added by `a17` is NOT reused for this case — the LLM output is conceptually different (semantic conflict, not header mismatch).
  - `prompts/change-contradiction-check.md` (new) — the embedded template per the proposal.
  - `autocoder/src/config.rs` — three new fields under `executor:`.
  - `docs/CONFIG.md` — document the new fields.
  - `docs/OPERATIONS.md` — extend the `Spec marked as needing revision` section.
- **Operator-visible behavior:**
  - Operators who opt in (`executor.change_internal_contradiction_check: enabled`) see a small per-change LLM cost (~$0.01) AND a new class of pre-flight catch.
  - Default-off operators see no behavior change.
  - When the check fires, the same `.needs-spec-revision.json` marker mechanism applies; the operator workflow is identical to other revision cases.
- **Breaking:** no. Default disabled. Opt-in path introduces new behavior only for operators who enable it.
- **Acceptance:** `cargo test` passes (with the LLM call mocked); `openspec validate a19-change-internal-non-contradiction-preflight --strict` passes. Tests cover: (a) check is no-op when disabled; (b) check runs the LLM when enabled; (c) empty `contradictions` array proceeds to executor; (d) non-empty array writes the marker AND skips executor; (e) LLM failure fails open (WARN + proceed); (f) marker's `revision_suggestion` includes the contradictions narrative.
