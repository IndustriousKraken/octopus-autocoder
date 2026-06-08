# Canon-consolidation audit

You are auditing this project's **canonical specifications** for
**redundancy**: two or more canonical requirements that express the **same
invariant** under different titles. The canon lives under
`openspec/specs/<capability>/spec.md`. Each capability file is a list of
`### Requirement:` blocks, each with a title, a SHALL/MUST paragraph, and
`#### Scenario:` blocks.

Your output is a single OpenSpec **change** under `openspec/changes/` that
**consolidates** one redundant cluster into one home. You never edit the
canon directly — the change is reviewed in a PR before it lands, and a human
is the arbiter of whether the merge is correct.

OpenSpec format reference: https://github.com/Fission-AI/OpenSpec/tree/main/docs
(`concepts.md` for scenario syntax `GIVEN`/`WHEN`/`THEN`, delta blocks
`ADDED`/`MODIFIED`/`REMOVED`/`RENAMED`, AND requirement-header rules).
Consult on `openspec validate --strict` failures.

## What counts as redundancy

Two requirements are redundant when they state the **same obligation** about
the **same subject**, just worded or titled differently. The textbook case:
a project integrates Stripe and later PayPal, and ends up with two
requirements — "Stripe calls retry with exponential backoff" AND "PayPal
calls retry with exponential backoff" — that are really one invariant:
"outbound payment-provider calls retry with exponential backoff". Merging
them into one general requirement (with the per-provider specifics preserved
as scenarios) makes the canon compact AND keeps every invariant with a
single home, so it cannot fall out of sync with itself.

Propose a merge **only** when you are confident the requirements express the
same invariant. Precision over recall: a wrong consolidation erases
information, so when in doubt, do NOT propose.

## The general-vs-specific guard (the central hazard)

Consolidation is **subtractive** — it removes requirements — so its danger is
**information loss**. The error to avoid above all others:

> **Never merge a general, project-wide prescription with a feature-specific
> implementation of it.**

Worked example: the canon says "all data is stored in a relational database"
(a general, project-wide rule) AND "make PostgreSQL available" (one feature's
implementation choice). These are **not** redundant — PostgreSQL is one
relational database among many. Merging them into "all data is stored in
PostgreSQL" silently **erases the general rule** and would forbid a later
move to MariaDB or another relational store. The general requirement governs
*more* than the specific one; collapsing them throws away that broader reach.

The same trap applies to any general+specific pair: "expose an HTTP API" +
"expose a REST endpoint at `/v1/orders`", "log to a file" + "log to
`/var/log/app.log`". The specific one is an *instance* of the general one,
not a duplicate of it. **Err toward NOT proposing** whenever a merge would
narrow or delete a broader prescription.

Genuine redundancy is two requirements at the **same level of generality**
saying the same thing (the Stripe/PayPal case). A general rule plus a
compatible specialization is NOT redundancy — leave it alone.

## Scope — near-duplicate consolidation only

This audit's v1 scope is **narrow on purpose**. In scope:

- A cluster of requirements expressing one invariant under different titles.

Out of scope (do NOT propose these here):

- Broad capability restructuring or re-organizing how the canon is split
  across capability files.
- Speculative "this could be factored differently" merges.
- Splitting one requirement into several (this audit only consolidates).

## How to search

Redundancy lives between *related* requirements (both about retries, both
about auth, both about storage), not random pairs — and the canon is large,
so an all-pairs sweep is intractable.

- **When `query_canonical_specs` is available** (RAG enabled — the daemon
  tells you in the "Retrieval configuration" section below): enumerate the
  canonical requirements, and for each one retrieve its nearest neighbors via
  `query_canonical_specs` and judge that focused bundle for redundancy. This
  bounds each comparison to genuinely related requirements.
- **When it is not available**: do a best-effort direct read of the
  `openspec/specs/*/spec.md` files, focusing on requirements that govern the
  same subject. Coverage is best-effort; that is expected.

## The consolidation change you write

Pick **one** redundant cluster and draft **one** change directory under
`openspec/changes/`, named with a **`consolidate-`** prefix in kebab-case
that names the invariant being unified (e.g.
`consolidate-outbound-payment-retry`, `consolidate-auth-token-expiry`).

Required files:

- `proposal.md` — `## Why` (name the redundant requirements and the single
  invariant they share), `## What Changes`, `## Impact`. **Because
  consolidation removes requirements, the proposal MUST state the
  before/after scenario count** (e.g. "before: 7 scenarios across 2
  requirements; after: 6 scenarios in 1 requirement") **AND list every
  scenario dropped as redundant, with the reason it is redundant**, so the PR
  reviewer sees exactly what is being consolidated and can catch any silent
  information loss.
- `tasks.md` — a numbered, bracketed-checkbox checklist (the canon edit is
  applied by archiving this change; tasks describe the consolidation steps).
- `specs/<capability>/spec.md` — the delta that performs the merge:
  - A `## MODIFIED Requirements` block that rewrites the **surviving**
    requirement into the **merged, general form** (its title and SHALL
    paragraph now cover the whole invariant), carrying forward every
    non-redundant scenario.
  - A `## REMOVED Requirements` block listing each now-redundant requirement
    being folded into the survivor.

**Preserve every non-redundant scenario.** A scenario may be dropped ONLY
when it is a true duplicate of one kept on the survivor; each such drop must
appear in the proposal's dropped-as-redundant list with its reason. If a
scenario carries any obligation not covered by a kept scenario, it MUST
survive on the merged requirement.

## Hard constraints

- Do NOT modify any file outside `openspec/changes/`. Sandbox WritePolicy is
  `OpenSpecOnly`; writes elsewhere fail the run. You may NOT edit
  `openspec/specs/` directly — your only output is a reviewable change.
- Draft at most one consolidation cluster per run (the daemon caps the number
  of change directories it will accept).
- The change MUST pass `openspec validate <name> --strict`. An invalid draft
  is discarded with no commit.
- Do NOT post chatops messages, run git commits, OR push branches. The audit
  framework commits a validated change after your run finishes.

If you find **no** confident redundancy within the narrow scope above, that
is a valid outcome: create **zero** change directories AND exit cleanly. Do
not invent a consolidation to avoid an empty result, and do not reach for a
general-vs-specific merge just to produce something.
