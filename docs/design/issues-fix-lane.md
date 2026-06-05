# Issues-fix lane — design

**Status:** specced as `a009-issues-lane-curated` (the lane mechanism + curated path) and `a010-issues-lane-hybrid-ingestion` (public-issue ingestion + the prompt-quarantine trust boundary, stacked on a009). Prerequisites `a000-harden-external-triggers` and `a002-single-pass-prompt-substitution` are archived. The forge-side issues ingestion (GitLab) remains Phase 3 of the forge work.

## Motivation

Not all work is a spec change. There are two distinct kinds:

- **Spec changes** — new or changed behavior. These belong in `openspec/changes/` and must carry a spec delta.
- **Corrections** — fixes to code that is already specified correctly: bug fixes and behavior-preserving refactors. These change no spec, so they have no delta. Today they have to be forced into `changes/` with a manufactured `project-documentation` requirement (as `a68` was), because OpenSpec rejects a delta-less change.

The issues lane gives corrections a first-class home, and provides a path to triage and fix reported GitHub issues — including reports from untrusted public authors — without weakening the trust model.

## Two lanes

| | `changes/` | `issues/` |
|---|---|---|
| Work | new or changed behavior | corrections (bug fixes, refactors, reported defects) |
| Spec delta | required | none |
| Artifact | `proposal.md` + `tasks.md` + `specs/<cap>/spec.md` | `issue.md` + `tasks.md` |
| Verified against | the delta | the **existing canon** |
| On archive | the spec is updated | audit trail only; spec untouched |

The verification asymmetry is the load-bearing distinction. A change asks *"does the code match the new delta?"* An issue asks *"does the fix make the code match the spec it already has?"* Both are verifiable predicates, and the issue path needs no new spec — so the verifier/reviewer machinery still applies, pointed at canon instead of a delta.

## Architecture: separate lanes over shared utilities

The two lanes are **separate implementations**, not one walker with an `is_issue` flag. A fault in one walker must not be able to reach the other lane's control flow or state. Shared leaf functionality is extracted into stateless utilities both lanes compose.

```
            shared utilities (stateless primitives)
   busy-marker · open_pr · archive · chatops_notify · queue-state I/O · workspace
            ▲                                    ▲
            │ compose                            │ compose
   ┌──────────────────┐                 ┌──────────────────┐
   │  changes walker  │                 │  issues walker   │
   │  own control flow│                 │  own control flow│
   │  own state file  │                 │  own state file  │
   │  verify vs DELTA │                 │  verify vs CANON │
   └──────────────────┘                 └──────────────────┘
        two independent drivers; a fault in one cannot corrupt the other
```

DRY on the leaf functions; decoupled on the orchestration. Lane-specific behavior (issue-only triage / dedup / promotion; change-only spec-delta validation) lives in each walker, not in shared branching.

## Queue administrator

No new coordinator is required. The per-repo serializer already exists — the busy-marker enforces one unit of work per repository at a time (same-repo strict blocking). The issues lane adds one precedence tier on top of the existing order (`a12` already established `changes` over `audits`):

```
        per-repo serializer (busy-marker) — one unit at a time, already enforced
                                  │
                each iteration, pick the highest-precedence READY unit:
                                  │
        issues/   >   changes/   >   audits
      (restore code   (new/changed   (regenerating
       → spec base)    behavior)      on cadence)
        within a lane: alphabetical
```

**Strict issue-precedence:** an issue always beats a change when both are ready. Anti-starvation is provided by the **promotion gate** (issues only enter the lane after a maintainer approves them — see Ingestion), not by a scheduling fairness rule. A fairness valve is deferred unless a change is observed starving behind a continuous issue stream; the promotion gate should make that impossible in practice.

## Ingestion: hybrid (curated is a subset)

- **Curated** — a maintainer commits `issues/<slug>/` directly. Repo write is the allowlist; there is no public surface.
- **Hybrid** — the bot triages public GitHub issues read-only (reusing scout's existing issue read, `scout.rs` / `features.scout.include_issues`), dedups against open and archived issues, drafts a candidate, and posts it to chatops. A maintainer **"send it"**s the candidate; only then does the bot write `issues/<slug>/` and queue it.

The "send it" promotion is the authorization gate: the public can **report**, but cannot **trigger** code work. It reuses the audit "send it" pattern. Curated is the hybrid path minus the auto-triage step.

## Trust boundary (quarantine)

The issues lane feeds untrusted issue bodies into a **code-writing** executor, unlike scout, which is read-only. Quarantine is therefore load-bearing here and is a required component of this lane (it was considered as a standalone change, "a001", and folded in here where it earns its keep). Defense in depth:

1. **Promotion gate** — untrusted content enters the fix path only after a maintainer approves the candidate.
2. **Prompt quarantine** — the issue body is delimited as DATA with an explicit untrusted-report framing. The task and scope come from the lane and the maintainer-approved classification, never from the body. The delimiter is robust (not a markdown fence the body can break), and single-pass substitution (`a002`) prevents `{{token}}` expansion of placeholder text inside the body.
3. **Human merge** — the PR is the final backstop.

Net effect: an injected issue body can at worst waste compute. It cannot trigger work (promotion gate) and cannot ship code (human merge).

## Issue artifact

```
issues/<slug>/
  issue.md     # the report (provenance noted when from a public author),
               # the diagnosis, and acceptance criteria — what "fixed" means
               # against the EXISTING spec
  tasks.md     # the fix steps
```

No `specs/` directory — that absence is the point. On completion the directory moves to `issues/archive/`, mirroring `changes/archive/`.

## Triage routing

Each report is classified:

- **Bug** — code has drifted from a spec that is itself correct → issues lane.
- **Behavior change** — the report actually wants new or changed behavior → routed to `changes/` as a proposal, not an issue.
- **Question / invalid / duplicate** — declined or deduped; no work queued.

## Execution and drift

- **Issue-flavored implementer prompt:** fix to match the existing spec; do not invent a spec change; if the fix actually requires changed behavior, kick it back to `changes/`.
- **Verify against existing canon** — the verifier/reviewer point at the spec, not a delta.
- **Drift handling is light.** Because issues run first, changes are usually authored against an already-corrected baseline. When an issue has already done a pending change's work, the implementer adapts. A change that then finds nothing to do may fail — which is the correct outcome for it. Where "already satisfied" is detectable, the lane may close the change cleanly (archive as a no-op) rather than firing a perma-stuck alert; a plain failure is an acceptable fallback. No rebase-precheck in v1.

## Reuse map

| Existing machinery | Used for |
|---|---|
| scout's issue read (`gh api …/issues`) | hybrid ingestion |
| chat-request-triage primitive (`build_chat_triage_prompt`, `propose` verb) | issue triage / classification |
| per-repo serializer (busy-marker) + `a12` precedence | the queue administrator |
| queue / PR-open / archive / chatops utilities | the shared utility module both lanes compose |

## Sequencing and dependencies

1. **`a000-harden-external-triggers`** — first. Closes the live hole where any GitHub commenter can trigger billed work. The issues lane's promotion gate is the same trust posture applied to a new surface.
2. **`a002-single-pass-prompt-substitution`** — prevents `{{token}}` expansion in injected issue bodies; a prerequisite for safe ingestion (it also fixes scout's existing `{{open_issues}}` path).
3. **The issues lane** — `a009` (lane + curated) then `a010` (hybrid ingestion + the prompt-quarantine control described above, stacked on a009).

## Open questions (deferred)

- **Fairness valve** — add a scheduling anti-starvation rule only if a change is observed starving behind a continuous issue stream. The promotion gate is expected to prevent this.
- **Rebase-precheck** for changes left stale by a shipped issue — deferred; light drift handling first.
- **"Already satisfied" clean-close** semantics versus plain failure — a detail to settle during implementation.
- **Scope of related cleanups** — per-hotspot versus project-wide (e.g. the wording-assertion test purge handled in `a68` for `polling_loop.rs` could become a project-wide sweep).
