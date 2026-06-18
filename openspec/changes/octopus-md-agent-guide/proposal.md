# OCTOPUS.md — an agent guide to the workflow conventions

## Why

A repository under autocoder management carries `openspec/`, `issues/`, and
(soon) a global-rules corpus — but an agent or human opening that repo has no
in-repo explanation of how those work. Autocoder's own agents get it from
injected prompts; everyone else (a coding assistant or speccing agent run
directly on the repo, a teammate) gets nothing. An `OCTOPUS.md` at the repo root
— the spot an `AGENTS.md` would occupy — closes that gap: it orients any agent to
the OpenSpec workflow, the issues format, and the rules format, and gives offline
agents enough OpenSpec inline to work without retrieval.

It also captures the spec-writing guardrails agents keep tripping over, the most
costly of which we just hit in production: an agent (a spec-implementing /
revision session) both **archived a change** and **folded its deltas into
`openspec/specs/`** mid-implementation — autocoder's post-merge job, done early,
which bypasses the `[out]` gate and double-applies on merge. OCTOPUS.md states
the rules plainly for any agent that reads it; for autocoder's own (gated,
sandboxed) agents the same rules are enforced by the gates and sandbox, which
this document does not replace.

The risk OCTOPUS.md must avoid is being one more place these conventions are
written down by hand and then drift — and the worst such place, since agents are
told to trust it without retrieval. So it is generated and owned by autocoder,
from the single source the prompts already use, and version-stamped.

## What Changes

- A `project-documentation` standard establishes OCTOPUS.md: its audience,
  contents (OpenSpec essentials inline + links; issues format; rules format), the
  spec-writing guardrails (no self-contradiction; no canon contradiction without
  an explicit `MODIFY`/`RENAME`/`REMOVE`; no spec-sync / apply-to-canon tasks; no
  direct `openspec/specs/` edits; no archiving — autocoder archives after
  implementation), the generated-not-hand-authored + version-stamped constraint,
  and discoverability via a managed `AGENTS.md` pointer that does not clobber an
  existing `AGENTS.md`.
- `autocoder install` writes OCTOPUS.md and the `AGENTS.md` pointer; a
  regeneration path rewrites it from the canonical format definitions
  (idempotent; sections reflect enabled features; OpenSpec section stamped to the
  installed `openspec` version).

## Impact

- Affected specs: `project-documentation` (ADD the OCTOPUS.md standard),
  `orchestrator-cli` (ADD the generator + `AGENTS.md` pointer).
- Affected code: the install flow (write OCTOPUS.md + AGENTS.md pointer), a
  regeneration entry point, and a shared "format definitions" source that both
  the generator and the agent prompts render from (so they cannot diverge).
- Sequencing: implement AFTER `single-file-issues` and `global-rules-gate` land,
  so OCTOPUS.md documents finalized formats rather than chasing them; the rules
  section is populated once the global-rules feature exists.
- This change documents the spec-writing guardrails; enforcing the no-archive /
  no-canon-edit invariant for autocoder's OWN spec-writing sessions (the PR that
  motivated this) is a separate sandbox/revert concern, not closed here.
