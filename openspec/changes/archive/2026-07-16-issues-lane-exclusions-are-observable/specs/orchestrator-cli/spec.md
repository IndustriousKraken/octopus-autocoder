## ADDED Requirements

### Requirement: Issues-lane exclusions are logged and stale locks are recovered
The issues lane's ready-list SHALL NOT skip a unit silently. Each pass that excludes a unit SHALL log the exclusion at INFO with the slug and the reason: `locked` (with the lock's age) for an `.in-progress` lock, or `parked` (with the marker's marked-at time) for a `.perma-stuck.json` marker — so an excluded issue is continuously visible in the daemon journal, not only at the moment its marker was written.

An `.in-progress` lock is transient by contract (it marks a unit actively being worked within an iteration); a lock that persists is a crash or kill leftover. The ready-list SHALL treat a lock whose age exceeds the existing busy-marker stale threshold (`executor.busy_marker_stale_threshold_secs` — no new configuration) as STALE: remove it, log a WARN naming the slug and the lock's age, AND post a chatops alert that the lock was recovered — after which the unit is selectable again on the same pass. A fresh lock (age within the threshold) excludes the unit exactly as before. Park markers are NEVER auto-removed — parking is an operator-owned state per the parking requirement; only its ongoing visibility is new.

#### Scenario: A parked issue's exclusion is visible every pass
- **WHEN** an issue carries a `.perma-stuck.json` marker AND the issues lane enumerates ready units
- **THEN** the pass logs the exclusion at INFO with the slug, `parked`, and the marker's marked-at time
- **AND** the unit is excluded from selection exactly as before

#### Scenario: A freshly locked issue is excluded and logged
- **WHEN** an issue carries an `.in-progress` lock younger than the busy-marker stale threshold
- **THEN** the pass logs the exclusion at INFO with the slug, `locked`, and the lock's age
- **AND** the unit is excluded from selection

#### Scenario: A stale lock is recovered instead of excluding forever
- **WHEN** an issue carries an `.in-progress` lock older than the busy-marker stale threshold (e.g. a daemon crash left it behind)
- **THEN** the ready-list removes the lock, logs a WARN naming the slug and the lock's age, AND posts a chatops alert that the stale lock was recovered
- **AND** the unit is selectable again (strict alphabetical precedence applies as usual)

#### Scenario: Park markers are never auto-removed
- **WHEN** an issue's `.perma-stuck.json` marker is older than any threshold
- **THEN** the marker is NOT removed by the ready-list
- **AND** only the operator's marker removal unparks the issue

## MODIFIED Requirements

### Requirement: Marker-clear operator commands accept wildcard targets
The marker-clear operator commands `clear-perma-stuck` AND `clear-revision` SHALL accept a wildcard target so an operator can clear markers in bulk without naming each change OR each repository. In addition to the exact form (`clear-<kind> <repo-substring> <change-slug>`), the parser SHALL recognize:

- `clear-<kind> <repo-substring> *` — clear EVERY marker of that kind in the one resolved repository.
- `clear-<kind> *` — clear EVERY marker of that kind across ALL configured repositories.

The literal `*` is a wildcard sentinel, NOT a change-slug NOR a repo-substring. Its ACCEPTANCE at the parser — recognized for these two verbs before the change-slug / repo-substring regex, exempt from those patterns — is defined by the chatops-manager "Argument sanitization at parser entry" requirement; THIS requirement defines what the sentinel DOES (the bulk clear below). A change-slug position is therefore either a sanitized slug OR `*`; a repo-substring position is either a sanitized substring OR `*`; every non-`*` argument is sanitized as the chatops-manager requirement specifies.

**`clear-perma-stuck` covers BOTH work lanes.** Its marker enumeration SHALL include the changes lane (`openspec/changes/<change>/.perma-stuck.json`) AND the issues lane in both of its forms (the in-directory `issues/<slug>/.perma-stuck.json` AND the single-file sibling `issues/<slug>.perma-stuck.json`) — a parked issue is clearable by the same verb whose park alert names the marker, and a sweep over a repository that carries ONLY issue-lane park markers reports them cleared, never "nothing to clear". The exact-target form resolves the slug against the changes lane first (the existing exact-or-prefix behavior, unchanged), THEN against issue units when no change matches; each cleared marker in any reply is labeled with its lane. `clear-revision` remains changes-lane-only — an issue carries no spec delta, so no `.needs-spec-revision.json` can exist for it.

The wildcard sweep is a DISTINCT operation from a single-target clear: when the target is `*`, the action enumerates the marker directories itself AND SHALL NOT pass `*` through the single-slug resolver defined by the "Partial change-slug resolution in marker-clearing control-socket actions" requirement. That resolver continues to govern ONLY non-`*` (single-target) clears, where it resolves an exact-or-prefix `change` value as before; `*` is intercepted ahead of it AND never reaches it. (This is why a single-slug resolver that has not been taught about `*` would otherwise return `NoMatch` for a literal `*` — the sweep path must branch before resolution.)

Bulk clearing SHALL be fail-loud, never silent: the reply SHALL enumerate what was cleared — each repository AND each unit (change or issue, labeled by lane) whose marker was removed — AND SHALL report a repository (or the whole fleet) that had no matching markers in EITHER lane as an explicit "nothing to clear" result rather than an empty reply. A per-repository failure (e.g. a workspace that cannot be read) SHALL NOT abort the sweep; it is reported alongside the successes. Wildcard clearing removes ONLY the named marker kind (`clear-perma-stuck` → `.perma-stuck.json`; `clear-revision` → `.needs-spec-revision.json`), matching the exact-form behavior, including `clear-perma-stuck`'s removal of an accompanying `.ignore-for-queue.json` when present (a changes-lane concept; issue units carry none).

#### Scenario: Wildcard clears all markers of a kind in one repo
- **WHEN** an operator posts `@<bot> clear-perma-stuck your-repo *` AND `your-repo` resolves to one repository
- **THEN** every `.perma-stuck.json` marker in that repository — changes lane AND issues lane, both issue forms — is removed
- **AND** the reply enumerates each unit whose marker was cleared, labeled by lane
- **AND** if the repository had no such markers in either lane, the reply says so explicitly

#### Scenario: A parked issue is cleared by the sweep, not reported as nothing-to-clear
- **WHEN** a repository's only `.perma-stuck.json` marker is on an issue unit (e.g. `issues/<slug>/.perma-stuck.json`)
- **AND** an operator posts `@<bot> clear-perma-stuck <repo> *`
- **THEN** the issue's marker is removed AND the reply names the issue, labeled as issues-lane
- **AND** the reply does NOT claim there was nothing to clear

#### Scenario: An exact-target clear reaches an issue marker when no change matches
- **WHEN** an operator posts `@<bot> clear-perma-stuck <repo> <slug>` AND no change directory matches the slug AND a parked issue unit does
- **THEN** the issue's `.perma-stuck.json` marker (in-directory or sibling, per its form) is removed
- **AND** the reply names the cleared issue, labeled as issues-lane

#### Scenario: clear-revision remains changes-lane-only
- **WHEN** an operator posts a `clear-revision` wildcard or exact-target command
- **THEN** only `openspec/changes/<change>/.needs-spec-revision.json` markers are considered
- **AND** issue units are not enumerated (no such marker can exist for them)

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
- **WHEN** an operator posts `@<bot> clear-perma-stuck your-repo a06-foo` (no wildcard) AND `a06-foo` matches a change
- **THEN** the behavior is exactly as before — the single named marker is cleared, with the same success AND not-found replies
