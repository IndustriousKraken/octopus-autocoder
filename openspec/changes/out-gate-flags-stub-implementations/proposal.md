# The [out] gate flags stubbed and deferred implementations as gaps

## Why

The project bans stubs — every change ships fully runnable, with no
placeholder returns, `todo!()`, or "deferred to a follow-up" early-returns
on a path the spec requires. The ban lives in the implementer prompt, but
implementers ignore it: a change lands with a key function stubbed or its
wiring deferred, the diff looks plausible, and the gap reaches working code.
A recent example shipped the read half of a control as a dead, unwired
function; the behavior the spec required was never reachable.

The `[out]` gate (code-implements-spec verification) is the post-executor
check positioned to catch exactly this — it already judges, requirement by
requirement, whether the implementation satisfies the spec delta. But its
charter is framed around absent behavior ("not implemented at all" /
"not fully honored"), not around behavior that is present-but-fake. A stub
that returns a hardcoded value, or a branch left unwired with a "wire this
up later" comment, can read as "implemented" to a verifier that is only
asking "is there code for this?" rather than "does this code actually do the
work?"

## What Changes

- The `orchestrator-cli` "Code-implements-spec verification (the [out] gate,
  advisory)" requirement is clarified: judging satisfaction explicitly
  includes judging that the required behavior is really implemented, not
  stubbed or deferred. Where the spec calls for working code, a stub
  (placeholder/hardcoded return, `todo!()`/`unimplemented!()`, an
  unconditional early-return skipping the required path, an unwired branch,
  a flag read but never acted on, OR an explicit deferral to a later change)
  is a gap — `missing` when wholly stubbed, `partial` when a required path
  is stubbed — reported with the stub as evidence. A new scenario pins this.
- The embedded `[out]` prompt (`prompts/code-implements-spec-check.md`) is
  strengthened to name the stub/deferral forms and instruct the verifier to
  treat them as gaps even when the spec delta does not separately say "do
  not stub."

The gate remains advisory (it annotates the PR and posts a heads-up; it
never blocks PR creation) — this change sharpens what it catches, not what
it does about it.

## Impact

- Affected specs: `orchestrator-cli` (MODIFY the [out] gate requirement —
  add the no-stubs judging criterion + a scenario).
- Affected code: `prompts/code-implements-spec-check.md` (the embedded
  prompt). No Rust logic changes — the gate's plumbing, verdict schema, and
  advisory behavior are unchanged; only the verifier's instructions sharpen.
- The gate is advisory, so this surfaces stubs in the PR body and a chatops
  heads-up; it does not block. Making a stub finding BLOCK (e.g. via the
  reviewer's `RequestChanges`) is a separate, larger change and is out of
  scope here.
