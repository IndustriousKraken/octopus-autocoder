//! Forge-provider abstraction (a007, Phase 1).
//!
//! Every forge **API** operation routes through the [`Forge`] trait, whose
//! concrete implementation is selected per repository by the repository
//! URL's host (see [`resolve`]). The trait surface is everything coupled to
//! the forge today: repository-URL parsing; PR/MR lifecycle (open, list-open,
//! find-by-head, draft handling); comment listing-since AND posting; review
//! posting; fork creation; commenter authorization; AND the push-only branch
//! hint.
//!
//! The git operations (clone, fetch, branch, commit, push) are deliberately
//! NOT part of this trait — they use the raw URL and the `origin` remote and
//! stay host-neutral in `git.rs`.
//!
//! This module provides the single [`GithubForge`] implementation, which
//! reproduces the current GitHub behavior exactly: the `github.rs` REST
//! shapes (now relocated to [`github`], the single source of truth for forge
//! REST calls), the `author_association`-based authorization gate (formerly
//! `revisions::is_comment_authorized`), AND the draft-PR handling (folded
//! into [`Forge::open_pr`]'s `draft` parameter — GitHub creates a PR as a
//! draft in one call, with the existing label fallback when the host rejects
//! the draft flag). A host with no registered provider resolves to a clear
//! error naming the host, preserving today's rejection of non-GitHub URLs
//! until a later change registers an additional provider.

pub mod github;
pub mod gitlab;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::code_reviewer::ReviewReport;
use crate::config::CommandAuthorizationConfig;
use github::{CreatedPr, IssueComment, OpenPr, PrSummary};

/// How a posted review maps onto the forge (a008). The reviewer's verdict is
/// provider-agnostic; each provider lowers it onto its own primitives.
/// `GithubForge` posts all three as a PR comment (GitHub's reviewer flow has
/// always been comment-based); `GitlabForge` maps `Approve` onto an MR
/// approval AND the other two onto an MR note (GitLab has no distinct
/// request-changes review state).
///
/// `#[allow(dead_code)]`: today's daily loop posts only the `RequestChanges`
/// reviewer-revision comment; `Approve` AND `Comment` complete the
/// provider-agnostic verdict mapping (exercised by the GitLab tests) AND are
/// wired by the Phase-3 GitLab reviewer integration.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewDecision {
    /// The review approves the change.
    Approve,
    /// The review asks for changes before merge.
    RequestChanges,
    /// The review is an informational comment (no verdict).
    Comment,
}

/// The authorization decision a forge assigns to a commenter for a
/// comment-sourced command (e.g. `@<bot> revise`). Default-deny: anything
/// not explicitly allowed is [`AuthLevel::Unauthorized`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthLevel {
    /// The commenter may dispatch comment-sourced commands.
    Authorized,
    /// The commenter may NOT dispatch comment-sourced commands.
    Unauthorized,
}

/// Every forge API operation autocoder performs. The concrete implementation
/// is selected per repository by URL host via [`resolve`]. Implementations
/// are `Send + Sync` so the daemon can hold a `Box<dyn Forge>` across `.await`
/// points and share it between tasks.
#[async_trait]
pub trait Forge: Send + Sync {
    /// Parse a repository URL into `(owner, repo)` — the GitHub-shaped
    /// project identity. Host validation is the resolver's job (see
    /// [`resolve`]); this returns the path components for the already-
    /// resolved provider.
    fn parse_repo(&self, url: &str) -> Result<(String, String)>;

    /// Open a pull/merge request. `draft` requests a draft PR; the provider
    /// owns the draft lifecycle (for GitHub: request-draft, and on a host
    /// that rejects the flag, retry non-draft AND apply a `do-not-merge`
    /// label). Returns the created PR's URL and number.
    #[allow(clippy::too_many_arguments)]
    async fn open_pr(
        &self,
        owner: &str,
        repo: &str,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
        token: &str,
        review_report: Option<&ReviewReport>,
        draft: bool,
    ) -> Result<CreatedPr>;

    /// List open PRs whose head and base match the given qualifiers. Used by
    /// the polling loop to skip an iteration when a PR is already pending.
    async fn list_open_prs(
        &self,
        owner: &str,
        repo: &str,
        head: &str,
        base: &str,
        token: &str,
    ) -> Result<Vec<OpenPr>>;

    /// Find open PRs whose head is `{head_owner}:{head_branch}`. Used by the
    /// revision dispatcher to locate bot-opened PRs to poll for triggers.
    async fn find_pr_by_head(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        head_owner: &str,
        head_branch: &str,
    ) -> Result<Vec<PrSummary>>;

    /// List issue comments on `pr_number` created at or after `since`.
    async fn list_comments_since(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        pr_number: u64,
        since: DateTime<Utc>,
    ) -> Result<Vec<IssueComment>>;

    /// Post an issue comment (the revision dispatcher's reply path).
    async fn post_comment(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        pr_number: u64,
        body: &str,
    ) -> Result<()>;

    /// Post a review, lowering `decision` onto the provider's review
    /// primitives (see [`ReviewDecision`]). `GithubForge` posts the body as a
    /// PR comment regardless of `decision` (preserving the comment-based
    /// reviewer flow); `GitlabForge` maps `Approve` onto an MR approval AND
    /// the other verdicts onto an MR note.
    async fn post_review(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
        body: &str,
        decision: ReviewDecision,
        token: &str,
    ) -> Result<()>;

    /// Create a fork of the upstream repository (idempotent on 2xx).
    async fn create_fork(
        &self,
        upstream_owner: &str,
        upstream_repo: &str,
        token: &str,
    ) -> Result<()>;

    /// Decide whether a comment-sourced command from `comment` is authorized,
    /// per `auth`. `GithubForge` applies the GitHub `author_association` gate
    /// exactly as the pre-extraction `revisions::is_comment_authorized` did.
    fn authorize(&self, comment: &IssueComment, auth: &CommandAuthorizationConfig) -> AuthLevel;

    /// The push-only branch hint: a URL the operator can open to review a
    /// pushed branch when PR creation was skipped (`auto_submit_pr: false`).
    fn branch_url(&self, owner: &str, repo: &str, branch: &str) -> String;
}

/// The GitHub forge provider. Holds the REST API base (`DEFAULT_API_BASE` in
/// production; a mockito server URL in tests) so the trait methods can thread
/// it into the relocated [`github`] REST functions without changing their
/// signatures.
pub struct GithubForge {
    api_base: String,
}

impl GithubForge {
    /// A `GithubForge` against the live GitHub REST API.
    pub(crate) fn new() -> Self {
        Self {
            api_base: github::DEFAULT_API_BASE.to_string(),
        }
    }

    /// A `GithubForge` against an explicit API base. Production callers that
    /// already hold a (possibly test-injected) `api_base` use this so the
    /// existing mockito-driven tests exercise the trait path unchanged.
    pub(crate) fn with_api_base(api_base: impl Into<String>) -> Self {
        Self {
            api_base: api_base.into(),
        }
    }
}

#[async_trait]
impl Forge for GithubForge {
    fn parse_repo(&self, url: &str) -> Result<(String, String)> {
        github::parse_repo_url(url)
    }

    async fn open_pr(
        &self,
        owner: &str,
        repo: &str,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
        token: &str,
        review_report: Option<&ReviewReport>,
        draft: bool,
    ) -> Result<CreatedPr> {
        github::create_pull_request_at(
            &self.api_base,
            owner,
            repo,
            head,
            base,
            title,
            body,
            token,
            review_report,
            draft,
        )
        .await
    }

    async fn list_open_prs(
        &self,
        owner: &str,
        repo: &str,
        head: &str,
        base: &str,
        token: &str,
    ) -> Result<Vec<OpenPr>> {
        github::list_open_prs_at(&self.api_base, owner, repo, head, base, token).await
    }

    async fn find_pr_by_head(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        head_owner: &str,
        head_branch: &str,
    ) -> Result<Vec<PrSummary>> {
        github::list_open_prs_for_head(&self.api_base, token, owner, repo, head_owner, head_branch)
            .await
    }

    async fn list_comments_since(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        pr_number: u64,
        since: DateTime<Utc>,
    ) -> Result<Vec<IssueComment>> {
        github::list_issue_comments_since(&self.api_base, token, owner, repo, pr_number, since).await
    }

    async fn post_comment(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        pr_number: u64,
        body: &str,
    ) -> Result<()> {
        github::post_issue_comment(&self.api_base, token, owner, repo, pr_number, body).await
    }

    async fn post_review(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
        body: &str,
        _decision: ReviewDecision,
        token: &str,
    ) -> Result<()> {
        // GitHub's reviewer flow is comment-based: every verdict is posted as
        // a PR comment, so `decision` does not change the request shape.
        github::create_issue_comment_at(&self.api_base, owner, repo, pr_number, body, token).await
    }

    async fn create_fork(
        &self,
        upstream_owner: &str,
        upstream_repo: &str,
        token: &str,
    ) -> Result<()> {
        github::create_fork_at(&self.api_base, upstream_owner, upstream_repo, token).await
    }

    fn authorize(&self, comment: &IssueComment, auth: &CommandAuthorizationConfig) -> AuthLevel {
        // Ported verbatim from the pre-extraction `revisions::is_comment_authorized`:
        // authorized when EITHER the author's `login` is in `allowed_users`
        // (case-insensitive, matching the bot-self filter convention) OR the
        // comment's `author_association` is in `allowed_associations`
        // (case-sensitive — GitHub associations are canonical uppercase). An
        // absent/unrecognized association on its own is unauthorized
        // (default-deny).
        let login = comment.user_login();
        if !login.is_empty()
            && auth
                .allowed_users
                .iter()
                .any(|u| u.eq_ignore_ascii_case(login))
        {
            return AuthLevel::Authorized;
        }
        match comment.author_association() {
            Some(assoc) if auth.allowed_associations.iter().any(|a| a == assoc) => {
                AuthLevel::Authorized
            }
            _ => AuthLevel::Unauthorized,
        }
    }

    fn branch_url(&self, owner: &str, repo: &str, branch: &str) -> String {
        // a26 shape: `https://github.com/<owner>/<repo>/tree/<branch>`.
        format!("https://github.com/{owner}/{repo}/tree/{branch}")
    }
}

/// Extract the host from a repository URL, accepting the SSH (`git@host:`),
/// HTTPS (`https://host/`, `http://host/`), AND `ssh://git@host/` forms.
/// Returns the bare host (e.g. `github.com`). Errors when the URL matches
/// none of the recognized shapes.
pub(crate) fn forge_host(url: &str) -> Result<String> {
    let trimmed = url.trim();
    let without_suffix = trimmed.strip_suffix(".git").unwrap_or(trimmed);

    if let Some(rest) = without_suffix.strip_prefix("git@") {
        // `git@host:owner/repo`
        if let Some((host, _)) = rest.split_once(':')
            && !host.is_empty()
        {
            return Ok(host.to_string());
        }
        return Err(unrecognized_url_error(url));
    }
    for scheme in ["https://", "http://", "ssh://git@", "ssh://"] {
        if let Some(rest) = without_suffix.strip_prefix(scheme) {
            let host = rest.split(['/', ':']).next().unwrap_or("");
            if !host.is_empty() {
                return Ok(host.to_string());
            }
        }
    }
    Err(unrecognized_url_error(url))
}

fn unrecognized_url_error(url: &str) -> anyhow::Error {
    anyhow!(
        "unrecognized repository URL `{url}`: expected an SSH (`git@host:owner/repo.git`) or \
         HTTPS (`https://host/owner/repo(.git)?`) form"
    )
}

/// `true` when `host` is a host this build registers a forge provider for.
/// Phase 1 registers GitHub only (`github.com`), matching today's
/// `parse_repo_url` acceptance set; GitHub Enterprise / GitLab hosts are
/// added by later changes.
fn is_github_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("github.com")
}

/// Resolve the forge provider for a repository, applying the a008 selection
/// precedence:
///
/// 1. An explicit per-repo `forge:` block is **authoritative** — `kind:
///    gitlab` selects [`gitlab::GitlabForge`]; `kind: github` selects
///    [`GithubForge`] (against the block's `api_base` when set, enabling
///    GitHub Enterprise).
/// 2. Absent a block, a `github.com` host resolves to [`GithubForge`].
/// 3. Otherwise no provider is registered for the host AND a clear
///    no-provider error is returned, directing the operator to declare a
///    `forge:` block.
///
/// GitLab is reachable ONLY via an explicit `forge: { kind: gitlab }` — there
/// is no host-sniffing fallback, so a GitLab-host URL with no block returns
/// the no-provider error rather than silently selecting GitLab.
pub(crate) fn resolve_forge(
    forge: Option<&crate::config::ForgeConfig>,
    url: &str,
) -> Result<Box<dyn Forge>> {
    use crate::config::ForgeKind;
    if let Some(cfg) = forge {
        return Ok(match cfg.kind {
            ForgeKind::Gitlab => Box::new(gitlab::GitlabForge::from_config(
                cfg.host.as_deref(),
                cfg.api_base.as_deref(),
                url,
            )),
            ForgeKind::Github => match cfg.api_base.as_deref() {
                Some(base) => Box::new(GithubForge::with_api_base(base.to_string())),
                None => Box::new(GithubForge::new()),
            },
        });
    }
    let host = forge_host(url)?;
    if is_github_host(&host) {
        Ok(Box::new(GithubForge::new()))
    } else {
        Err(no_provider_error(&host, url))
    }
}

/// The clear no-provider error for an unregistered host. Names the host AND
/// directs the operator to declare a per-repo `forge:` block.
fn no_provider_error(host: &str, url: &str) -> anyhow::Error {
    anyhow!(
        "no forge provider is registered for host `{host}` (from `{url}`): declare a per-repo \
         `forge:` block to select a provider (e.g. `forge: {{ kind: gitlab, host: {host} }}`)"
    )
}

/// Resolve the forge for `url` and parse its `(owner, repo)`. The single
/// entry point for repo-owner resolution: it validates the host (rejecting
/// unsupported hosts by name) before parsing the project path. Threads an
/// optional per-repo `forge:` block so GitLab / GHE repositories parse via
/// the configured provider.
pub(crate) fn parse_repo_with(
    forge: Option<&crate::config::ForgeConfig>,
    url: &str,
) -> Result<(String, String)> {
    resolve_forge(forge, url)?.parse_repo(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_reviewer::{ReviewReport, ReviewVerdict};
    use crate::config::CommandAuthorizationConfig;

    fn auth(users: &[&str], assocs: &[&str]) -> CommandAuthorizationConfig {
        CommandAuthorizationConfig {
            allowed_users: users.iter().map(|s| s.to_string()).collect(),
            allowed_associations: assocs.iter().map(|s| s.to_string()).collect(),
            decline_comment: false,
        }
    }

    fn comment(login: &str, assoc: Option<&str>) -> IssueComment {
        IssueComment {
            id: 1,
            body: "@bot revise do thing".to_string(),
            user: Some(github::IssueCommentUser {
                login: login.to_string(),
            }),
            created_at: chrono::Utc::now(),
            author_association: assoc.map(|s| s.to_string()),
        }
    }

    // ---- Provider selection by URL host (spec scenarios) ----

    #[test]
    fn github_host_resolves_to_github_forge() {
        // Both SSH and HTTPS GitHub forms resolve and parse to (owner, repo).
        for url in [
            "git@github.com:owner/repo.git",
            "https://github.com/owner/repo.git",
            "https://github.com/owner/repo",
        ] {
            let forge = resolve_forge(None, url).expect("github URL must resolve");
            let (owner, repo) = forge.parse_repo(url).expect("github URL must parse");
            assert_eq!((owner.as_str(), repo.as_str()), ("owner", "repo"), "{url}");
        }
    }

    #[test]
    fn unsupported_host_errors_naming_the_host() {
        // `Box<dyn Forge>` is not `Debug`, so match rather than `expect_err`.
        let err = match resolve_forge(None, "https://gitlab.example.com/owner/repo.git") {
            Ok(_) => panic!("non-github host must not resolve"),
            Err(e) => e,
        };
        let msg = format!("{err:#}");
        assert!(
            msg.contains("gitlab.example.com"),
            "error must name the offending host; got: {msg}"
        );
    }

    #[test]
    fn parse_repo_with_rejects_unsupported_host_by_name() {
        let err = parse_repo_with(None, "https://gitlab.example.com/owner/repo.git")
            .expect_err("non-github host must not parse");
        assert!(format!("{err:#}").contains("gitlab.example.com"));
    }

    // ---- a008 §4.1: per-repo forge-block selection precedence ----

    fn gitlab_block(api_base: Option<&str>) -> crate::config::ForgeConfig {
        crate::config::ForgeConfig {
            kind: crate::config::ForgeKind::Gitlab,
            host: Some("gitlab.example.com".into()),
            api_base: api_base.map(|s| s.to_string()),
            token: None,
            token_env: None,
        }
    }

    fn github_block(api_base: Option<&str>) -> crate::config::ForgeConfig {
        crate::config::ForgeConfig {
            kind: crate::config::ForgeKind::Github,
            host: None,
            api_base: api_base.map(|s| s.to_string()),
            token: None,
            token_env: None,
        }
    }

    #[test]
    fn explicit_gitlab_block_selects_gitlab_forge() {
        // A `forge: { kind: gitlab }` block makes a non-github host resolve,
        // AND parsing uses GitLab's namespace/project semantics (a nested
        // path that GithubForge would reject).
        let url = "https://gitlab.example.com/group/subgroup/project.git";
        let cfg = gitlab_block(None);
        let forge = resolve_forge(Some(&cfg), url).expect("gitlab block must resolve");
        let (owner, repo) = forge.parse_repo(url).expect("gitlab URL must parse");
        assert_eq!((owner.as_str(), repo.as_str()), ("group/subgroup", "project"));
    }

    #[test]
    fn github_com_without_block_selects_github_forge() {
        let url = "https://github.com/owner/repo";
        let forge = resolve_forge(None, url).expect("github.com must resolve without a block");
        let (owner, repo) = forge.parse_repo(url).expect("github URL must parse");
        assert_eq!((owner.as_str(), repo.as_str()), ("owner", "repo"));
    }

    #[test]
    fn gitlab_host_without_block_returns_no_provider_error() {
        // No host-sniffing: a GitLab-host URL with no block does NOT select
        // GitLab; it returns the no-provider error directing the operator to
        // declare a `forge:` block.
        let err = match resolve_forge(None, "https://gitlab.example.com/owner/repo.git") {
            Ok(_) => panic!("gitlab host without a block must not resolve"),
            Err(e) => e,
        };
        let msg = format!("{err:#}");
        assert!(msg.contains("gitlab.example.com"), "must name host; got: {msg}");
        assert!(msg.contains("forge:"), "must direct to a forge block; got: {msg}");
    }

    #[tokio::test]
    async fn github_block_with_api_base_drives_github_against_ghe() {
        // `kind: github` + a self-hosted `api_base` selects GithubForge and
        // hits the GitHub REST shape against that endpoint.
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/repos/owner/repo/pulls")
            .with_status(201)
            .with_body(r#"{"html_url":"https://ghe.example.com/owner/repo/pull/1","number":1}"#)
            .create_async()
            .await;
        let cfg = github_block(Some(&server.url()));
        let forge = resolve_forge(Some(&cfg), "https://ghe.example.com/owner/repo")
            .expect("github GHE block must resolve");
        let pr = forge
            .open_pr("owner", "repo", "agent-q", "main", "t", "b", "tok", None, false)
            .await
            .expect("GHE PR create should succeed");
        assert_eq!(pr.number, 1);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn gitlab_block_open_pr_uses_gitlab_api_shape() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/projects/group%2Fproj/merge_requests")
            .with_status(201)
            .with_body(r#"{"iid":1,"web_url":"u"}"#)
            .create_async()
            .await;
        let cfg = gitlab_block(Some(&server.url()));
        let forge = resolve_forge(Some(&cfg), "https://gitlab.example.com/group/proj.git")
            .expect("gitlab block must resolve");
        forge
            .open_pr("group", "proj", "agent-q", "main", "t", "b", "tok", None, false)
            .await
            .expect("gitlab MR create should succeed");
        mock.assert_async().await;
    }

    #[test]
    fn forge_host_extracts_each_url_form() {
        assert_eq!(forge_host("git@github.com:o/r.git").unwrap(), "github.com");
        assert_eq!(
            forge_host("https://github.com/o/r").unwrap(),
            "github.com"
        );
        assert_eq!(
            forge_host("ssh://git@gitlab.acme.io/o/r.git").unwrap(),
            "gitlab.acme.io"
        );
        assert!(forge_host("not-a-url").is_err());
    }

    // ---- Commenter authorization rides the forge (spec scenario, 6.4) ----

    #[test]
    fn authorize_matches_allowed_association() {
        let f = GithubForge::new();
        let a = auth(&[], &["OWNER", "MEMBER"]);
        assert_eq!(
            f.authorize(&comment("alice", Some("MEMBER")), &a),
            AuthLevel::Authorized
        );
        assert_eq!(
            f.authorize(&comment("bob", Some("CONTRIBUTOR")), &a),
            AuthLevel::Unauthorized
        );
    }

    #[test]
    fn authorize_matches_allowed_user_case_insensitively() {
        let f = GithubForge::new();
        let a = auth(&["Trusted-User"], &["OWNER"]);
        // allowed_users is case-insensitive; association need not match.
        assert_eq!(
            f.authorize(&comment("trusted-user", Some("NONE")), &a),
            AuthLevel::Authorized
        );
    }

    #[test]
    fn authorize_association_match_is_case_sensitive() {
        let f = GithubForge::new();
        let a = auth(&[], &["OWNER"]);
        // GitHub associations are canonical uppercase; a lowercase value
        // does NOT match (matches the pre-extraction gate exactly).
        assert_eq!(
            f.authorize(&comment("x", Some("owner")), &a),
            AuthLevel::Unauthorized
        );
    }

    #[test]
    fn authorize_absent_association_and_unknown_user_is_unauthorized() {
        let f = GithubForge::new();
        let a = auth(&["someone"], &["OWNER"]);
        assert_eq!(
            f.authorize(&comment("stranger", None), &a),
            AuthLevel::Unauthorized
        );
    }

    #[test]
    fn branch_url_is_github_tree_url() {
        let f = GithubForge::new();
        assert_eq!(
            f.branch_url("owner", "repo", "agent-q"),
            "https://github.com/owner/repo/tree/agent-q"
        );
    }

    // ---- GithubForge exercised through the trait reproduces REST shapes ----

    #[tokio::test]
    async fn open_pr_through_trait_posts_expected_request() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/repos/owner/repo/pulls")
            .match_header("authorization", "Bearer testtoken")
            .match_body(mockito::Matcher::JsonString(
                r#"{"title":"t","body":"b","head":"agent-q","base":"main"}"#.to_string(),
            ))
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_body(r#"{"html_url":"https://github.com/owner/repo/pull/1","number":1}"#)
            .create_async()
            .await;
        let forge = GithubForge::with_api_base(server.url());
        let pr = forge
            .open_pr(
                "owner", "repo", "agent-q", "main", "t", "b", "testtoken", None, false,
            )
            .await
            .expect("PR creation through the trait should succeed");
        assert_eq!(pr.html_url, "https://github.com/owner/repo/pull/1");
        assert_eq!(pr.number, 1);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn list_open_prs_through_trait_parses_response() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock(
                "GET",
                "/repos/owner/repo/pulls?state=open&head=owner%3Aagent-q&base=main",
            )
            .with_status(200)
            .with_body(
                r#"[{"number":42,"html_url":"https://github.com/owner/repo/pull/42"}]"#,
            )
            .expect(1)
            .create_async()
            .await;
        let forge = GithubForge::with_api_base(server.url());
        let prs = forge
            .list_open_prs("owner", "repo", "owner:agent-q", "main", "testtoken")
            .await
            .expect("list through trait should succeed");
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0].number, 42);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn find_pr_by_head_through_trait_parses_response() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", mockito::Matcher::Any)
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("state".into(), "open".into()),
                mockito::Matcher::UrlEncoded("head".into(), "owner:agent-q".into()),
            ]))
            .with_status(200)
            .with_body(
                r#"[{"number":7,"title":"x","state":"open","html_url":"u","created_at":"2026-01-01T00:00:00Z","head":{"ref":"agent-q"},"base":{"ref":"main"}}]"#,
            )
            .expect(1)
            .create_async()
            .await;
        let forge = GithubForge::with_api_base(server.url());
        let prs = forge
            .find_pr_by_head("t", "owner", "repo", "owner", "agent-q")
            .await
            .expect("find_pr_by_head through trait should succeed");
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0].number, 7);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn post_comment_through_trait_uses_token_auth_header() {
        // `post_comment` (the revision-reply path) uses the `token <pat>`
        // auth header, exactly as the pre-extraction `post_issue_comment`.
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/repos/owner/repo/issues/42/comments")
            .match_header("authorization", "token testtoken")
            .match_body(mockito::Matcher::JsonString(r#"{"body":"hi"}"#.to_string()))
            .with_status(201)
            .with_body(r#"{"id":1}"#)
            .expect(1)
            .create_async()
            .await;
        let forge = GithubForge::with_api_base(server.url());
        forge
            .post_comment("testtoken", "owner", "repo", 42, "hi")
            .await
            .expect("post_comment through trait should succeed");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn post_review_through_trait_uses_bearer_auth_header() {
        // `post_review` (the reviewer path) uses the `Bearer <pat>` auth
        // header, exactly as the pre-extraction `create_issue_comment`.
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/repos/owner/repo/issues/9/comments")
            .match_header("authorization", "Bearer testtoken")
            .with_status(201)
            .with_body(r#"{"id":2}"#)
            .expect(1)
            .create_async()
            .await;
        let forge = GithubForge::with_api_base(server.url());
        forge
            .post_review(
                "owner",
                "repo",
                9,
                "review body",
                ReviewDecision::RequestChanges,
                "testtoken",
            )
            .await
            .expect("post_review through trait should succeed");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn create_fork_through_trait_posts_to_forks_endpoint() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/repos/upstream/repo/forks")
            .match_header("authorization", "Bearer testtoken")
            .with_status(202)
            .with_body(r#"{"full_name":"bot/repo"}"#)
            .create_async()
            .await;
        let forge = GithubForge::with_api_base(server.url());
        forge
            .create_fork("upstream", "repo", "testtoken")
            .await
            .expect("create_fork through trait should succeed");
        mock.assert_async().await;
    }

    /// Silence the unused-import lint guard: `ReviewReport`/`ReviewVerdict`
    /// are used to prove `open_pr` accepts a review report through the trait.
    #[tokio::test]
    async fn open_pr_through_trait_appends_review_section() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/repos/owner/repo/pulls")
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"body":"base\n\n## Code Review\n\nVERDICT"}"#.to_string(),
            ))
            .with_status(201)
            .with_body(r#"{"html_url":"u","number":3}"#)
            .create_async()
            .await;
        let report = ReviewReport {
            verdict: ReviewVerdict::Pass,
            markdown: "VERDICT".to_string(),
            concerns: Vec::new(),
            per_change_sections: Vec::new(),
            attribution: None,
        };
        let forge = GithubForge::with_api_base(server.url());
        forge
            .open_pr(
                "owner",
                "repo",
                "agent-q",
                "main",
                "t",
                "base",
                "testtoken",
                Some(&report),
                false,
            )
            .await
            .expect("PR creation with review report should succeed");
        mock.assert_async().await;
    }
}
