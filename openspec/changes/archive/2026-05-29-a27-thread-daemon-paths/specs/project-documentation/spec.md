## ADDED Requirements

### Requirement: `docs/STATE-LAYOUT.md` "Path resolution rule" describes DaemonPaths threading AND `docs/test-reliability.md` records the resolved test-fallback caveat
`docs/STATE-LAYOUT.md`'s "Path resolution rule" section SHALL be updated to describe the threading model that replaces the prior `paths::current()` global. The section SHALL state:

1. The daemon constructs exactly one `DaemonPaths` value at startup (in the entrypoint module).
2. Every consumer of path information receives that value via constructor OR function parameter.
3. There is no process-global accessor; the formerly-existing `paths::current()` AND its helpers have been removed.
4. Tests construct their own `DaemonPaths` via `test_daemon_paths()` for per-test isolation; concurrent tests cannot collide on disk because each test's fixtures live under its own tempdir.
5. A CI scanner (extending the `a10` path-literals audit) blocks reintroduction of the removed global accessors anywhere under `autocoder/src/`.

`docs/test-reliability.md`'s disposition-table row for the `a10` caveat (the residual test-fallback issue) SHALL be updated:

- Status changes from "left for follow-up" to `fixed-in-a27`.
- The resolution paragraph names the threading refactor AND points the reader at the updated `docs/STATE-LAYOUT.md` "Path resolution rule" section for the architectural detail.

The `rm -rf /tmp/autocoder/` cleanup hint added by `a10` SHALL be removed (the shared location no longer accumulates fixtures, so the hint is obsolete).

#### Scenario: STATE-LAYOUT.md describes the threading model
- **WHEN** a maintainer reads `docs/STATE-LAYOUT.md`'s "Path resolution rule" section
- **THEN** the section names the single-construction-site rule (daemon entrypoint)
- **AND** the section names the parameter/constructor-threading rule for consumers
- **AND** the section explicitly states there is no `paths::current()` global
- **AND** the section names the CI scanner that prevents reintroduction

#### Scenario: STATE-LAYOUT.md describes per-test isolation
- **WHEN** the same maintainer reads further in the section
- **THEN** the section describes `test_daemon_paths()` as the canonical per-test construction helper
- **AND** the section names the isolation property (concurrent tests use disjoint tempdirs)

#### Scenario: test-reliability.md marks the caveat resolved
- **WHEN** the maintainer reads `docs/test-reliability.md`'s disposition table
- **THEN** the row covering the test-fallback issue is tagged `fixed-in-a27`
- **AND** the resolution paragraph names the threading refactor AND links to STATE-LAYOUT.md

#### Scenario: Obsolete cleanup hint removed
- **WHEN** the maintainer reads `docs/test-reliability.md`
- **THEN** the `rm -rf /tmp/autocoder/` cleanup hint is no longer present
- **AND** any prose that referenced it has been adjusted OR removed
