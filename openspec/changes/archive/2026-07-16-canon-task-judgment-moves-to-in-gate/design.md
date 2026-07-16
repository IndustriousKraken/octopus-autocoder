## Context

`preflight/canon_editing_tasks.rs::directs_canon_edit` flags a task when it contains any mutation-verb token AND any canon-target token (`openspec/specs/` substring, `canonical spec` substring, or the bare word `canon`) — with no binding between the verb and its object. Three false positives in three weeks: "Add a `## Roadmap items` section to OCTOPUS.md", prose *mentioning* an `openspec/specs/` path a prompt should READ, and "Update `docs/CHATOPS.md`'s documentation (the project-documentation canon requires…)". Each blocked a valid change post-push with a `.needs-spec-revision.json` marker and cost an operator round-trip; the third was initially misattributed to gate-model quality because the alert is indistinguishable from an agent verdict. The project's own contributing standard — control-plane code "assembles inputs and surfaces the agent's verdict but synthesizes none" — cuts against a keyword matcher synthesizing revision verdicts, even deny-direction ones. Meanwhile the `[in]` gate runs an agentic session at the same pipeline position with the same marker/alert/halt plumbing.

## Goals / Non-Goals

**Goals:**
- Prose that mentions canon never blocks a change; a task that genuinely directs a canon edit is still caught pre-executor.
- One judgment surface (the `[in]` gate) instead of a heuristic racing an agent to the same verdict.

**Non-Goals:**
- Removing `a17`'s mechanical spec-delta archivability check — header comparison against canon is a structural fact, exactly what mechanical checks are for. Only the *semantic* task-prose scan moves.
- Making the `[in]` gate mandatory. It stays opt-in; the disabled-gate backstop is the archive-time duplicate-requirement abort (also a mechanical fact) plus the implementer prompt's existing code-and-tests-only instruction.
- Changing the operator flow for true positives (same marker, same alert, same `clear-revision`).

## Decisions

- **Move, don't tune.** A narrower heuristic was considered (flag only literal `openspec/specs/` paths): it still false-flagged a read-only path mention in July, because "names a path" ≠ "directs an edit to it". Argument binding is parsing; parsing prose is the gate's job. No token list survives contact with a project whose vocabulary is canon-saturated.
- **Ride `submit_contradictions` with an optional `canon_editing_tasks` field** rather than a second tool or session. One session already reads the change; adding `tasks.md` to its reading list and one optional array to its schema is the smallest diff that keeps fail-closed semantics intact (schema-validated at the MCP layer, correctable mid-session).
- **Cost accounting.** The mechanical scan was free and ran unconditionally; the gate check adds no session (the `[in]` session already runs where enabled) and a marginal prompt/reading cost. Deployments without the gate trade pre-executor detection for archive-time detection — the failure that motivated the scan (archive abort → perma-stuck) is now survivable: parking alerts loudly and the doom-loop-era re-implement bugs are fixed.
- **Delete the module, keep the marker shape.** `canon_editing_tasks` findings render into `revision_suggestion` like contradiction findings; the marker's structured populations are unchanged except that the pre-flight-specific population source disappears with its producer.

## Risks / Trade-offs

- [The gate agent misses a genuinely canon-directing task] → The archive abort still catches it deterministically at fold time, now with loud parking; the net risk is a delayed failure for one change, versus the current certainty of recurring false blocks on valid changes.
- [Gate-disabled deployments lose pre-executor detection entirely] → Stated in the migration; those deployments already accepted less pre-executor scrutiny by disabling the gate, and the implementer prompt + archive backstop remain.
- [Schema addition breaks older gate sessions] → The field is optional; an agent that never sends it yields exactly today's behavior.
