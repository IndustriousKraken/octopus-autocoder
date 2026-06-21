If `OCTOPUS.md` exists at the repository root, read it before you start: it
states this repo's in-repo workflow protocols (the issues format, the OpenSpec
change format, the canon/archive ownership rules, and the gate model). When
`OCTOPUS.md` is absent, skip this with no further action.

You are surveying an existing codebase to identify the discrete capabilities that warrant their own OpenSpec spec. Your output is a curated list of proposed capabilities; you do NOT write the specs themselves — that happens in a later step. The operator will review your list AND decide which capabilities to spec.

## Operator guidance

{{guidance}}

## Repository context

- Repo URL: `{{repo_url}}`
- Workspace `README.md`:

{{readme}}

- `docs/` files in the workspace:

{{docs_listing}}

- Code-symbol overview (top-level public items, build-system metadata where applicable):

{{symbols_overview}}

- Capabilities ALREADY specced under `openspec/specs/` (DO NOT propose any of these — they are out of scope):

{{already_specced}}

## Process

1. **Read the code structure.** Use Glob, Grep, AND Read to map the major modules, public functions, AND user-visible behaviors. Trace from entry points (CLI flags, HTTP routes, public APIs, chatops verbs, scheduled jobs — whatever applies) into the code that implements them.
2. **Identify cohesive slices of behavior.** A capability is one cohesive concern with a discernible boundary: one logical responsibility, a recognizable surface area (its own modules / its own entry points), AND a level of complexity that fits in one OpenSpec spec (5-10 requirements is the sweet spot).
3. **Propose capability boundaries.** Each proposed item names what's IN, what's OUT (so the operator can see how concerns split between siblings), AND the source-tree paths that constitute its surface area.
4. **Stay within the cap.** Produce AT MOST `{{max_capabilities}}` items. Fewer is fine if the codebase has fewer cohesive boundaries.

## Output

Return a **JSON array AND nothing else** (no prose, no markdown fences around the JSON). Each item has the shape:

```json
{
  "id": 1,
  "slug": "scheduler",
  "summary": "One-line description.",
  "scope_in": "Short paragraph naming what's IN this capability (modules, behaviors).",
  "scope_out": "Short paragraph naming related concerns that DO NOT belong in this capability (handed off to other capabilities OR explicitly out-of-scope).",
  "source_modules": ["src/scheduler/", "src/cron/"],
  "estimated_complexity": "small"
}
```

Field rules:

- `id` — 1-indexed sequential integer matching the array order.
- `slug` — proposed capability slug. MUST match `^[a-z][a-z0-9-]*$` (lowercase, hyphenated, starts with a letter). MUST NOT be one of the already-specced capabilities listed above.
- `summary` — single sentence, ≤140 chars, no trailing punctuation required.
- `scope_in` — short paragraph (1-3 sentences). Names the behaviors AND modules included.
- `scope_out` — short paragraph (1-3 sentences). Names adjacent concerns explicitly excluded so the operator sees how siblings divide responsibility.
- `source_modules` — list of source-tree paths (directory or file) where the capability lives.
- `estimated_complexity` — exactly one of `"small"` | `"medium"` | `"large"`. Heuristic the operator uses to decide whether to split a large capability.

## Anti-noise rules

- **Do NOT propose capabilities for already-specced areas.** The list above is authoritative — any slug appearing there is OFF-LIMITS.
- **Do NOT split a single cohesive behavior across multiple capabilities.** If two slices share state, naming, OR an obvious abstraction, they belong together.
- **Do NOT bundle unrelated behaviors into one capability.** A capability is a slice of behavior, not a grab bag of "stuff in `src/util/`".
- **Aim for small-to-medium complexity (5-10 requirements each).** If a candidate would require >10 requirements, flag it as `large` so the operator can decide whether to split.
- **Tone: candidates for consideration, NOT ranked recommendations.** The operator decides what gets specced. Don't editorialize in `summary` or `scope_in`; describe what the capability IS, not how important it is.
- **Cap at `{{max_capabilities}}` items.** Returning more is a validation failure.
