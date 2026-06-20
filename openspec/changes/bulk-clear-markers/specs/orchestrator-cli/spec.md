## ADDED Requirements

### Requirement: Marker-clear operator commands accept wildcard targets
The marker-clear operator commands `clear-perma-stuck` AND `clear-revision` SHALL accept a wildcard target so an operator can clear markers in bulk without naming each change OR each repository. In addition to the exact form (`clear-<kind> <repo-substring> <change-slug>`), the parser SHALL recognize:

- `clear-<kind> <repo-substring> *` — clear EVERY marker of that kind in the one resolved repository.
- `clear-<kind> *` — clear EVERY marker of that kind across ALL configured repositories.

The literal `*` is a wildcard sentinel, NOT a change-slug NOR a repo-substring: the parser SHALL recognize it for these two verbs BEFORE applying the change-slug / repo-substring regex, so it is exempt from those patterns. The argument-sanitization rule continues to govern every NON-`*` argument unchanged; this requirement adds the `*` sentinel for the marker-clear verbs only. A change-slug position is therefore either a sanitized slug OR `*`; a repo-substring position is either a sanitized substring OR `*`.

Bulk clearing SHALL be fail-loud, never silent: the reply SHALL enumerate what was cleared — each repository AND each change/marker removed — AND SHALL report a repository (or the whole fleet) that had no matching markers as an explicit "nothing to clear" result rather than an empty reply. A per-repository failure (e.g. a workspace that cannot be read) SHALL NOT abort the sweep; it is reported alongside the successes. Wildcard clearing removes ONLY the named marker kind (`clear-perma-stuck` → `.perma-stuck.json`; `clear-revision` → `.needs-spec-revision.json`), matching the exact-form behavior, including `clear-perma-stuck`'s removal of an accompanying `.ignore-for-queue.json` when present.

#### Scenario: Wildcard clears all markers of a kind in one repo
- **WHEN** an operator posts `@<bot> clear-perma-stuck your-repo *` AND `your-repo` resolves to one repository
- **THEN** every `.perma-stuck.json` marker in that repository is removed
- **AND** the reply enumerates each change whose marker was cleared
- **AND** if the repository had no such markers, the reply says so explicitly

#### Scenario: Fleet-wide wildcard clears across all repositories
- **WHEN** an operator posts `@<bot> clear-revision *`
- **THEN** every `.needs-spec-revision.json` marker across ALL configured repositories is removed
- **AND** the reply enumerates, per repository, the changes whose markers were cleared
- **AND** a repository with no matching markers is reported as such, not omitted silently

#### Scenario: The wildcard token is exempt from the slug and repo regexes
- **WHEN** the parser encounters `*` in the change-slug position OR the repo-substring position of a marker-clear verb
- **THEN** it treats `*` as the wildcard sentinel rather than rejecting it as a malformed argument
- **AND** every non-`*` argument is still sanitized per the argument-sanitization rule

#### Scenario: A per-repository failure does not abort the fleet sweep
- **WHEN** a fleet-wide wildcard clear runs AND one repository's markers cannot be read or removed
- **THEN** the sweep continues across the remaining repositories
- **AND** the reply reports the failed repository AND cause alongside the repositories that were cleared

#### Scenario: The exact-target forms are unchanged
- **WHEN** an operator posts `@<bot> clear-perma-stuck your-repo a06-foo` (no wildcard)
- **THEN** the behavior is exactly as before — the single named marker is cleared, with the same success AND not-found replies
