## Why

`app-under-test-e2e` gave autocoder the ability to verify a change by running the application, but nothing tells the agents who write specs that the ability exists. A speccing agent that does not know it can require a runnable test writes prose assertions instead — "the form is wired correctly" rather than "WHEN the user submits the form THEN a confirmation appears" — and a prose assertion is exactly what a static reader passes and a running application fails.

That is the gap `app-under-test-e2e` was built to close, and it stays open for any repository whose specs are authored without knowing about it. The in-repo guide is the one document every agent working in a managed repository reads, and it already carries the analogous note for the local `verify` pre-check, so the precedent for how to document a capability there is established.

## What Changes

- OCTOPUS.md gains a short "End-to-end verification" section stating that a repository MAY be configured so autocoder verifies a change by running the application; that behavior described by scenarios is then expected to be verified by an executed end-to-end test rather than asserted in prose; that the end-to-end command's exit code is the authoritative signal; and that a new end-to-end test is replayed against the pre-change tree and expected to fail there.
- The section states this is per-repository operator configuration the reader cannot enable from inside the repository, AND that its absence leaves every other protocol in the guide unchanged — so a reader in an unconfigured repository is not misled into expecting behavior that will not happen.
- The content is added to the `OCTOPUS_MD` guide constant, which is the single deterministic source every managed repository is provisioned from. The on-disk copy in this repository is refreshed from it rather than edited independently.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `project-documentation`: OCTOPUS.md gains an end-to-end verification section, following the existing `OCTOPUS.md documents the local verify pre-check` requirement's precedent of one requirement per guide topic.

## Impact

- `autocoder/src/octopus_guide.rs`: a new section in the `OCTOPUS_MD` constant, plus a content test asserting it by meaning (matching the style of the existing verify-pre-check test) rather than by full-string match.
- `OCTOPUS.md` at this repository's root: refreshed from the constant so the provisioner's staleness check (`is_current`) stays satisfied and no pass rewrites it.
- Operator-visible change: every managed repository's guide gains the section on its next provisioning pass. Repositories without an `app_under_test` block are unaffected in behavior — the section explicitly says so.
