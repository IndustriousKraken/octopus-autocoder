# Tasks

## 1. Add the verify section to the seeded guide

- [x] 1.1 In `autocoder/src/octopus_guide.rs`, add a section to the `OCTOPUS_MD` content source — e.g. `## Verify a change locally before pushing` — that tells readers they MAY run `autocoder verify <change-slug>` before pushing, that it runs the same `[in]` / `[canon]` / `[rules]` gates the daemon runs pre-executor against the working-tree change, that it is read-only, and that it is a feedback accelerator, NOT a replacement for the server gates (which remain the fail-closed enforcement, run against fresher canon and covering every contributor).
- [x] 1.2 In the same section, note that `verify` ships in the autocoder binary and is usable without the daemon via the check-only install, AND that a gate reporting it "could not run" (fail-closed — model unconfigured, transport error, or no rule corpus) is an environment/config condition, not a spec defect (fix the config, not the change).

## 2. Tests

- [x] 2.1 Assert the provisioned `OCTOPUS.md` bytes (the `OCTOPUS_MD` source / `agents_md_region` output) contain the verify section — that a reader is told to run `autocoder verify <change-slug>` and that it is a feedback accelerator, not a replacement for the server gates. Derive the assertion from the produced content, not a brittle full-string match.

## 3. Provisioning is unchanged

- [x] 3.1 Confirm no change to the provisioning mechanism: the stale-comparison still uses the same single source bytes, so managed repos (including this repo's own root `OCTOPUS.md`) are re-provisioned with the enlarged content through the existing push + PR flow. No separate migration is needed.
