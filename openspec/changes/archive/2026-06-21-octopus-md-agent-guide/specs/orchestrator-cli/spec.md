## ADDED Requirements

### Requirement: The daemon provisions OCTOPUS.md via a dedicated pull request, per-repo configurable
The daemon SHALL provision `OCTOPUS.md` AND the `AGENTS.md` reference to it into a managed repository through the established push + pull-request flow — the SAME path any change rides — NOT during server provisioning (`autocoder install`, which writes host config, a systemd unit, AND a system user, and never touches a managed repository's working tree), AND NOT via a commit on the base branch outside a pull request.

**Where the write happens.** The daemon SHALL write `OCTOPUS.md` AND refresh the `AGENTS.md` reference ON THE AGENT BRANCH — after the per-iteration base sync checks out `base_branch` AND recreates the agent branch (the daemon's per-pass workspace preparation performs the base checkout, `git pull --ff-only`, AND `git::recreate_branch` for the agent branch, AFTER `ensure_initialized` has cloned/fetched the workspace; pass work happens on the recreated agent branch thereafter). Writing on the agent branch, after base sync, is the only placement that survives the per-pass dirty-recovery step: a file written UNTRACKED before base sync is wiped by `attempt_dirty_workspace_recovery` (`git reset --hard origin/<base>` + `git clean -fd`), AND a commit landed on `base_branch` diverges from `origin/<base>` so the next `git pull --ff-only` fails, the same dirty-recovery reset discards it, AND it violates the canon prohibition on base-branch commits outside a pull request.

**How it reaches the base branch.** The daemon SHALL stage the two files (`git::add_all`) AND commit them on the agent branch (`git::commit`), then ride the established push + PR-creation path (`git::push_force_with_lease` of the agent branch, then the PR-open step). The provisioning commit reaches the base branch only when the resulting pull request is merged — never by a direct base-branch commit. This bootstrap pull request MAY ride a pass that also carries change work, OR be opened on its own when no other unit is pending; either way the two files travel as ordinary committed content on the agent branch.

**Honoring the per-repo PR toggle.** The daemon SHALL honor the repository's `auto_submit_pr` setting on this provisioning, exactly as it does for change work: when `auto_submit_pr` is `true` (the default) it opens a pull request carrying the two files; when `auto_submit_pr` is `false` it pushes the agent branch carrying the two files AND surfaces the branch (the `BranchPushedNoPr` outcome) without opening a pull request, leaving the operator to open it after local review.

**Guide-provisioning toggle — global default, per-repo override.** Whether the daemon provisions the guide SHALL be a per-repository decision. A global default (`features.octopus_guide.enabled`, defaulting to ENABLED) sets the fleet-wide behavior, AND each repository MAY override it with its own setting; the EFFECTIVE value for a repository is its own override when set, else the global default. The default is ON because the in-repo agent guide benefits every managed repository the operator owns; the per-repo override exists so an operator can turn it OFF for a specific repository — one where they do not want metafiles, OR a third-party repository they contribute to where adding metafiles is not their decision — WITHOUT affecting the rest of the fleet. When the effective value is DISABLED for a repository, the daemon SHALL NEVER write `OCTOPUS.md` or `AGENTS.md` for it AND SHALL open no bootstrap pull request.

**Idempotency — no needless pull request.** Provisioning SHALL be idempotent. When the guide is already present AND current on the base branch (the committed `OCTOPUS.md` matches the content the daemon would generate AND the managed `AGENTS.md` region is present and current), the daemon SHALL do nothing: it writes nothing, commits nothing, AND opens no pull request, so no empty PR AND no churn is produced. The daemon SHALL write the files AND open the bootstrap pull request ONLY when the guide is missing OR out of date on the base branch. When the daemon refreshes the `AGENTS.md` reference it SHALL leave any other `AGENTS.md` content the repository carries untouched; an `AGENTS.md` that does not yet exist is created carrying only the reference.

The committed `OCTOPUS.md` SHALL state the in-repo workflow protocols per the `project-documentation` requirement `Managed repos carry a committed OCTOPUS.md agent guide` (the issues protocol, the OpenSpec change protocol, the canon/archive ownership rules, AND the gate model).

#### Scenario: A bootstrap pull request is opened when the guide is missing and provisioning is enabled
- **WHEN** the daemon processes a repository whose `features.octopus_guide` flag is enabled AND whose base branch has no `OCTOPUS.md` (or a stale one), with `auto_submit_pr` at its default `true`
- **THEN** it writes `OCTOPUS.md` AND the managed `AGENTS.md` reference on the recreated agent branch (after base sync), commits both, pushes the agent branch, AND opens a pull request carrying the two files
- **AND** the files reach the base branch only when that pull request is merged — the daemon makes no base-branch commit outside a pull request

#### Scenario: BranchPushedNoPr when auto_submit_pr is false
- **WHEN** the daemon would provision the guide for a repository whose `features.octopus_guide` flag is enabled but whose `auto_submit_pr` is `false`
- **THEN** it writes AND commits the two files on the agent branch AND pushes that branch, but opens no pull request (the `BranchPushedNoPr` outcome), surfacing the branch so the operator can open the pull request after local review

#### Scenario: No write and no pull request when provisioning is disabled
- **WHEN** the daemon processes a repository whose effective guide-provisioning value (its per-repo override when set, else the global default) is disabled
- **THEN** it writes neither `OCTOPUS.md` nor `AGENTS.md` AND opens no bootstrap pull request for them, leaving the repository's tree untouched by this feature

#### Scenario: A per-repo override disables provisioning for one repository while the fleet default leaves others enabled
- **WHEN** the global default is ENABLED but one repository carries its own guide-provisioning override set to disabled
- **THEN** the daemon provisions the guide for the other repositories (whose effective value is the enabled global default) but writes nothing AND opens no bootstrap pull request for the overridden repository

#### Scenario: No pull request when the guide is already current
- **WHEN** the daemon processes a repository whose base branch already carries an `OCTOPUS.md` matching what the daemon would generate AND a current managed `AGENTS.md` reference
- **THEN** it writes nothing, commits nothing, AND opens no bootstrap pull request — no empty PR, no churn

#### Scenario: Provisioning does not write the managed-repo guide
- **WHEN** `autocoder install` runs to provision a host
- **THEN** it does NOT write `OCTOPUS.md` into any managed repository's working tree (the guide is provisioned through the daemon's push + pull-request flow, not by host provisioning)
