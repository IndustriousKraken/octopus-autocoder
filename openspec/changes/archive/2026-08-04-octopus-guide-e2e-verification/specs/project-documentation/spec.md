# project-documentation delta: app-under-test-e2e

## ADDED Requirements

### Requirement: OCTOPUS.md documents end-to-end verification when a repository is configured for it
Beyond the protocols listed in the `Managed repos carry a committed OCTOPUS.md agent guide` requirement, OCTOPUS.md SHALL also tell readers that a repository MAY be configured so that autocoder verifies a change by RUNNING the application, not only by reading the diff.

OCTOPUS.md SHALL state that where this is configured: behavior described by a change's scenarios is expected to be verified by an end-to-end test that actually exercises it rather than asserted in prose; the end-to-end command's EXIT CODE is the authoritative signal; AND a new end-to-end test is replayed against the pre-change tree and expected to FAIL there, so a test that passes without the change is reported rather than accepted.

OCTOPUS.md SHALL state that this is per-repository operator configuration, so a reader cannot enable it from inside the repository, AND that its absence changes nothing about the protocols above.

The guide is a SINGLE deterministic source: this content lives in the guide constant that provisions every managed repository, so a repository's on-disk copy is refreshed from it rather than edited in place.

#### Scenario: Guide names the executable-verification expectation
- **WHEN** the provisioned OCTOPUS.md content is inspected
- **THEN** it states that scenarios are verified by an executed end-to-end test rather than asserted in prose where the repository is configured for it
- **AND** it names the exit code as the authoritative signal

#### Scenario: Guide names the red-green expectation
- **WHEN** the provisioned OCTOPUS.md content is inspected
- **THEN** it states that a new end-to-end test is replayed against the pre-change tree AND is expected to fail there

#### Scenario: Guide states the reader cannot enable it in-repo
- **WHEN** the provisioned OCTOPUS.md content is inspected
- **THEN** it states that end-to-end verification is per-repository operator configuration
- **AND** that its absence leaves every other protocol in the guide unchanged
