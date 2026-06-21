## ADDED Requirements

### Requirement: Marker-clear operator commands accept wildcard targets
The marker-clear operator commands `clear-perma-stuck` AND `clear-revision` SHALL accept a wildcard target so an operator can clear markers in bulk without naming each change OR each repository. In addition to the exact form (`clear-<kind> <repo-substring> <change-slug>`), the parser SHALL recognize:

- `clear-<kind> <repo-substring> *` — clear EVERY marker of that kind in the one resolved repository.
- `clear-<kind> *` — clear EVERY marker of that kind across ALL configured repositories.

The literal `*` is a wildcard sentinel, NOT a change-slug NOR a repo-substring. Its ACCEPTANCE at the parser — recognized for these two verbs before the change-slug / repo-substring regex, exempt from those patterns — is defined by the chatops-manager "Argument sanitization at parser entry" requirement; THIS requirement defines what the sentinel DOES (the bulk clear below). A change-slug position is therefore either a sanitized slug OR `*`; a repo-substring position is either a sanitized substring OR `*`; every non-`*` argument is sanitized as the chatops-manager requirement specifies.

The wildcard sweep is a DISTINCT operation from a single-target clear: when the target is `*`, the action enumerates the marker directories itself AND SHALL NOT pass `*` through the single-slug resolver defined by the "Partial change-slug resolution in marker-clearing control-socket actions" requirement. That resolver continues to govern ONLY non-`*` (single-target) clears, where it resolves an exact-or-prefix `change` value as before; `*` is intercepted ahead of it AND never reaches it. (This is why a single-slug resolver that has not been taught about `*` would otherwise return `NoMatch` for a literal `*` — the sweep path must branch before resolution.)

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

#### Scenario: A per-repository failure does not abort the fleet sweep
- **WHEN** a fleet-wide wildcard clear runs AND one repository's markers cannot be read or removed
- **THEN** the sweep continues across the remaining repositories
- **AND** the reply reports the failed repository AND cause alongside the repositories that were cleared

#### Scenario: The wildcard sweep bypasses the single-slug resolver
- **WHEN** a marker-clear action receives the wildcard target `*`
- **THEN** it branches to the sweep enumeration BEFORE invoking the single-slug resolver (`resolve_change_prefix`), so `*` is never resolved as an exact-or-prefix `change` value
- **AND** the single-slug resolver is invoked ONLY for non-`*` targets, where its exact-or-prefix behavior is unchanged

#### Scenario: The exact-target forms are unchanged
- **WHEN** an operator posts `@<bot> clear-perma-stuck your-repo a06-foo` (no wildcard)
- **THEN** the behavior is exactly as before — the single named marker is cleared, with the same success AND not-found replies
