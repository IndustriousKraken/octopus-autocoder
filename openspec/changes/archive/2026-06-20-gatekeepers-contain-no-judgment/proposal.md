# Gatekeepers contain no judgment — the verdict is the agent's or the failed state

## Why

The existing standard "Control-plane gatekeepers fail closed, never to a passing
verdict" guards one axis: an inability to run must not become a pass. It does not
guard a second, equally fail-open axis — the code manufacturing the verdict
itself.

A gatekeeper can satisfy every fail-closed clause and still pass work nothing
evaluated. Concretely: an agent-backed gate runs, the agent is invoked, it returns
a genuine "pass" verdict — no error, no default, no zero-item aggregation — yet
nothing was actually judged, because the code decided in advance there was nothing
to evaluate and framed the agent's prompt and verdict schema so that "pass" was the
only answer the agent could give. The agent ran; its judgment was theater. That is
fail-open by construction, and the current standard does not name it.

The two failure modes are distinct and both must be closed:
- **Inability-to-run → pass** (already covered): an error, a missing result, a
  zero-item aggregation becomes approve/verified/no-findings.
- **Code-manufactured verdict** (this change): the code derives the outcome by
  inspecting the inputs ("the materials are absent, so pass"), OR constrains the
  agent's options so a failing verdict cannot be expressed.

The principle that closes both: a gatekeeper's verdict is ALWAYS a genuine agent
evaluation or the explicit failed state — never anything the code synthesized. The
code's only jobs are to initialize to the failed state, assemble the inputs, invoke
the agent, and surface the agent's verdict verbatim.

## What Changes

- MODIFY the `project-documentation` requirement "Control-plane gatekeepers fail
  closed, never to a passing verdict" to add the judgment-ownership dimension: two
  clauses (the verdict is the agent's, surfaced verbatim, with no code-synthesized
  verdict; and the agent is never given an option set that forecloses failure) and
  two scenarios. The existing fail-closed clauses and scenarios are preserved
  unchanged — this extends the one gatekeeper standard rather than adding a
  parallel one, so there is a single home for "what a gatekeeper is."
- The developer-facing record (`CONTRIBUTING.md`, "Control-plane gatekeepers fail
  closed") gains the same dimension.

## Impact

- Affected specs: `project-documentation` (MODIFY the gatekeeper requirement).
- Affected docs: `CONTRIBUTING.md` (extend the gatekeeper section).
- No runtime code in this change. This is the standard; bringing each gatekeeper
  into conformance is separate work, sequenced after this lands so the rewrites
  have a contract to conform to. Known non-conformances to address as their own
  changes: the `[out]` gate manufacturing a pass when no spec-delta is found (it
  must instead fail to run, and locate the delta at its archived path), the
  reviewer's verdict mechanism requiring structured detail to fail, and any
  surfacing layer that converts an agent's failing/discarded verdict into silence
  or pass.
