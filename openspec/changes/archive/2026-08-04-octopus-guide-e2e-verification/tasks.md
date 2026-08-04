## 1. Guide content

- [x] 1.1 Add an "End-to-end verification (when this repository is configured for it)" section to the `OCTOPUS_MD` constant in `autocoder/src/octopus_guide.rs` — NOT to the on-disk `OCTOPUS.md`, which the provisioner rewrites from the constant and would clobber.
- [x] 1.2 The section states: scenarios are verified by an executed end-to-end test rather than asserted in prose; the end-to-end command's EXIT CODE is the authoritative signal; a new end-to-end test is replayed against the pre-change tree AND expected to fail there.
- [x] 1.3 The section states this is per-repository operator configuration the reader cannot enable in-repo, AND that its absence leaves every other protocol in the guide unchanged.
- [x] 1.4 Refresh this repository's own `OCTOPUS.md` from the constant so it matches byte-for-byte and `is_current` stays true (no provisioning churn on the next pass).

## 2. Tests

- [x] 2.1 Add a guide-content test asserting the section by MEANING (key phrases), matching the style of the existing `octopus_md_documents_local_verify_precheck` test rather than a brittle full-string match.
- [x] 2.2 Confirm the existing provisioning tests still pass — in particular that an already-current guide writes nothing and makes no commit, and a stale one is rewritten and committed.
- [x] 2.3 Run the full `cargo test --all-features` suite.
