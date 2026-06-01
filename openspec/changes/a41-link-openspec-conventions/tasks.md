# Implementation tasks

## 1. Agent-facing prompts — add OpenSpec docs pointer

Each prompt below gains a one-line pointer to OpenSpec's upstream documentation, placed near the prompt's existing "you are working with OpenSpec content" framing. The pointer's content shape:

> OpenSpec conventions reference: https://github.com/Fission-AI/OpenSpec/tree/main/docs — `concepts.md` covers scenario syntax (`GIVEN`/`WHEN`/`THEN`), delta format (`ADDED`/`MODIFIED`/`REMOVED`/`RENAMED`), AND requirement-header rules; `getting-started.md` shows worked examples. Consult these when in doubt about format.

The exact wording MAY adapt to each prompt's voice. The load-bearing parts are the upstream URL AND a topical hint (one of `GIVEN`, `WHEN`, `scenario`, `delta`, OR `Requirement`) so the regression test can verify presence without pinning prose.

- [ ] 1.1 `prompts/implementer.md` — insert the pointer near the existing OpenSpec mention on line 3 (`a Git project that uses OpenSpec for change management`).
- [ ] 1.2 `prompts/implementer-revision.md` — same shape, inserted near the prompt's OpenSpec framing.
- [ ] 1.3 `prompts/chat-request-triage.md` — same.
- [ ] 1.4 `prompts/audit-triage.md` — same.
- [ ] 1.5 `prompts/missing-tests-audit.md` — same.
- [ ] 1.6 `prompts/security-bug-audit.md` — same.
- [ ] 1.7 `prompts/brownfield-draft.md` — same.
- [ ] 1.8 `prompts/scout.md` — same.

## 2. Human-facing pointer — `docs/README.md`

- [ ] 2.1 Add a new bullet to `docs/README.md` (under the existing `## Internals` section OR a new `## Contributing` section near the bottom — choose whichever produces the lower-friction reading order for a contributor scanning the file):
  ```
  - [OpenSpec conventions](https://github.com/Fission-AI/OpenSpec/tree/main/docs) — upstream spec-format reference. This project follows OpenSpec for change management. The `concepts.md` AND `getting-started.md` pages cover scenario syntax (`GIVEN`/`WHEN`/`THEN`), delta format (`ADDED`/`MODIFIED`/`REMOVED`/`RENAMED`), AND requirement-header rules. Consult these when drafting a new `openspec/changes/<slug>/` proposal.
  ```
- [ ] 2.2 Phrasing rule: no kitsch (no exclamation marks, no "tip:" framing, no faux-friendly hooks) per the project's existing documentation tone.

## 3. Regression test — repo-grep pointers

- [ ] 3.1 Add a single integration-style test (or extend an existing prompts-presence test if one exists) at `autocoder/tests/openspec_pointers.rs` (OR an existing tests file if there's a natural home — check for `prompts_presence_test.rs` AND similar before creating a new file) that asserts each of the nine files contains the substring `https://github.com/Fission-AI/OpenSpec` AND at least one of the topical hints. Failure message names the offending file path AND the missing hint, so a future contributor removing the pointer sees a clear "you broke a41" signal.
- [ ] 3.2 Test SHALL read the prompt files via `std::fs::read_to_string` at test time (NOT include_str! embedding), so the test catches divergence between source files AND any embedded copies.
- [ ] 3.3 Test SHALL be deterministic — no network, no clock, no env mutation. Each file check is independent AND the test fails on first missing pointer with a clear path AND missing-hint diagnostic.

## 4. Acceptance gate

- [ ] 4.1 `cargo test` passes for the autocoder crate, including the new pointer regression test.
- [ ] 4.2 `openspec validate a41-link-openspec-conventions --strict` passes.
- [ ] 4.3 Manual spot-check: visit https://github.com/Fission-AI/OpenSpec/tree/main/docs in a browser AND confirm `concepts.md` AND `getting-started.md` are present at the linked path. If upstream renames or restructures the docs directory, update the pointer text in this change before merge.
