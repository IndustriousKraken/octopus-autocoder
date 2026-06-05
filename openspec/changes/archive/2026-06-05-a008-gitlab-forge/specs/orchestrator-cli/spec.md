# orchestrator-cli — delta for a008-gitlab-forge

## ADDED Requirements

### Requirement: Per-repo forge config block
A repository MAY declare an explicit `forge:` block that selects AND configures its forge provider, with fields `kind` (`github` | `gitlab`), `host`, an optional `api_base`, AND a token route. Provider selection SHALL follow this precedence: (1) an explicit `forge:` block is authoritative; (2) absent a block, a `github.com` host resolves to `GithubForge`; (3) otherwise no provider is registered for the host AND the clear no-provider error is returned (per the `Forge provider abstraction` requirement).

GitLab SHALL be selected ONLY via an explicit `forge: { kind: gitlab }` — there is NO host-sniffing fallback to GitLab, so a GitLab-host URL without a `forge:` block returns the no-provider error rather than silently selecting GitLab. Existing GitHub configurations are unchanged AND need no `forge:` block. The `api_base` field additionally supports GitHub Enterprise: `kind: github` with a self-hosted `host`/`api_base` uses the GitHub shape against the non-`github.com` endpoint. The forge block's token route SHALL supply the provider's token through the existing token-routing mechanism.

#### Scenario: Explicit GitLab block selects GitlabForge
- **WHEN** a repository declares `forge: { kind: gitlab, host, token }`
- **THEN** its forge operations are served by `GitlabForge` against the configured host/`api_base`
- **AND** the configured token route supplies the GitLab token

#### Scenario: No forge block defaults to GitHub
- **WHEN** a repository on `github.com` has no `forge:` block
- **THEN** it resolves to `GithubForge` exactly as before
- **AND** no `forge:` block is required

#### Scenario: GitHub Enterprise via api_base
- **WHEN** a repository declares `forge: { kind: github, host, api_base }` for a self-hosted GitHub endpoint
- **THEN** it resolves to `GithubForge` using the GitHub shape against that `api_base`

#### Scenario: GitLab host without a block does not auto-select GitLab
- **WHEN** a repository URL has a non-`github.com` host AND no `forge:` block
- **THEN** the no-provider error is returned (directing the operator to declare a `forge:` block)
- **AND** GitLab is NOT selected by host inference
