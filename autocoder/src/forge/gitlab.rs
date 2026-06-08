//! GitLab forge provider (a008, Phase 2).
//!
//! [`GitlabForge`] implements the [`Forge`](super::Forge) trait against
//! GitLab's REST API (`/api/v4`), making a GitLab-hosted repository
//! first-class for the daily loop: autonomous merge-request creation, the
//! reviewer, the revision loop, and authorized `@<bot>` triggers.
//!
//! Key GitLab-shape differences from GitHub, encoded here:
//!
//! - **Project `:id`.** GitLab addresses a project by the URL-encoded
//!   `namespace/project` path (`group%2Fsubgroup%2Fproject`), supporting
//!   nested groups. [`parse_gitlab_url`] extracts the host AND the path;
//!   [`project_id`] URL-encodes it.
//! - **Merge requests** are addressed by their per-project `iid` (mapped
//!   onto the trait's `pr_number`), not GitHub's global PR number.
//! - **Draft state** is a `Draft:` title prefix, NOT a flag — `set_pr_draft`
//!   toggles the prefix via `PUT`.
//! - **Reviews.** GitLab has no request-changes review state: an approve
//!   verdict maps onto an MR approval; request-changes AND comment map onto
//!   an MR note.
//! - **Authorization** maps the commenter's project access level — Developer
//!   (30) and above is authorized; Reporter (20) AND Guest (10) are denied —
//!   mirroring the GitHub `author_association` gate.

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::github::{
    CreatedPr, ForgeIssue, IssueComment, IssueCommentUser, OpenPr, PrRefSummary, PrSummary,
};
use super::{AuthLevel, Forge, ReviewDecision};
use crate::code_reviewer::ReviewReport;
use crate::config::CommandAuthorizationConfig;

/// GitLab's SaaS host, used when neither a `forge.host` nor an inferable
/// repository-URL host is available.
pub(crate) const DEFAULT_HOST: &str = "gitlab.com";

/// GitLab member access level for Developer — the floor at which a commenter
/// may dispatch comment-sourced commands. Maintainer (40) AND Owner (50) sit
/// above it; Reporter (20) AND Guest (10) below. `#[allow(dead_code)]`:
/// referenced by the (forward-compat) access-level authorization gate.
#[allow(dead_code)]
pub const ACCESS_DEVELOPER: i64 = 30;

/// The GitLab forge provider. Holds the REST API base (`https://<host>/api/v4`
/// in production; a mockito server URL in tests) AND the web base
/// (`https://<host>`) used by [`Forge::branch_url`].
pub struct GitlabForge {
    api_base: String,
    web_base: String,
}

impl GitlabForge {
    /// A `GitlabForge` against the live GitLab REST API for `host`
    /// (e.g. `gitlab.com` or a self-hosted `gitlab.example.com`).
    pub(crate) fn new(host: &str) -> Self {
        let web_base = format!("https://{host}");
        Self {
            api_base: format!("{web_base}/api/v4"),
            web_base,
        }
    }

    /// A `GitlabForge` against an explicit API base (a mockito server URL in
    /// tests, or an operator-declared `api_base`). The web base is the API
    /// base with a trailing `/api/v4` trimmed so `branch_url` still points at
    /// the human-facing host.
    pub(crate) fn with_api_base(api_base: impl Into<String>) -> Self {
        let api_base = api_base.into();
        let web_base = api_base
            .strip_suffix("/api/v4")
            .unwrap_or(&api_base)
            .to_string();
        Self { api_base, web_base }
    }

    /// Build a `GitlabForge` from a per-repo `forge:` block. Precedence for
    /// the endpoint: an explicit `api_base` wins; else `host`; else the host
    /// inferred from the repository `url`; else `gitlab.com`.
    pub(crate) fn from_config(host: Option<&str>, api_base: Option<&str>, url: &str) -> Self {
        if let Some(base) = api_base {
            return Self::with_api_base(base.to_string());
        }
        let host = host
            .map(|h| h.to_string())
            .or_else(|| super::forge_host(url).ok())
            .unwrap_or_else(|| DEFAULT_HOST.to_string());
        Self::new(&host)
    }

    /// Map a GitLab member access level onto an authorization decision:
    /// Developer (30) and above is [`AuthLevel::Authorized`]; anything below
    /// (Reporter 20, Guest 10, or non-membership) is
    /// [`AuthLevel::Unauthorized`]. The pure decision behind
    /// [`GitlabForge::authorize_member`]. `#[allow(dead_code)]`: exercised by
    /// tests AND wired by the Phase-3 GitLab `@<bot>` trigger gate.
    #[allow(dead_code)]
    pub fn authorize_access_level(level: i64) -> AuthLevel {
        if level >= ACCESS_DEVELOPER {
            AuthLevel::Authorized
        } else {
            AuthLevel::Unauthorized
        }
    }

    /// Read the commenter's project access level via
    /// `GET /projects/:id/members/all/:user_id` AND authorize Developer (30)
    /// and above. A `404` (not a member) maps to
    /// [`AuthLevel::Unauthorized`] rather than an error — non-membership is a
    /// deny, not a failure. `#[allow(dead_code)]`: exercised by tests AND
    /// wired by the Phase-3 GitLab `@<bot>` trigger gate.
    #[allow(dead_code)]
    pub async fn authorize_member(
        &self,
        owner: &str,
        repo: &str,
        user_id: u64,
        token: &str,
    ) -> Result<AuthLevel> {
        let id = project_id(owner, repo);
        let url = format!("{}/projects/{id}/members/all/{user_id}", self.api_base);
        let resp = reqwest::Client::new()
            .get(&url)
            .header("PRIVATE-TOKEN", token)
            .header("User-Agent", "openspec-autocoder")
            .send()
            .await
            .map_err(|e| anyhow!("gitlab members GET failed: {e}"))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(AuthLevel::Unauthorized);
        }
        if !status.is_success() {
            let snippet = body_snippet(resp).await;
            return Err(anyhow!(
                "gitlab members GET {owner}/{repo} user {user_id} returned {status}: {snippet}"
            ));
        }
        #[derive(Deserialize)]
        struct Member {
            access_level: i64,
        }
        let member: Member = resp
            .json()
            .await
            .map_err(|e| anyhow!("gitlab members response decode failed: {e}"))?;
        Ok(Self::authorize_access_level(member.access_level))
    }
}

#[async_trait]
impl Forge for GitlabForge {
    fn parse_repo(&self, url: &str) -> Result<(String, String)> {
        let (_host, owner, repo) = parse_gitlab_url(url)?;
        Ok((owner, repo))
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
        let id = project_id(owner, repo);
        let source_branch = strip_head_owner(head);
        let title = if draft {
            ensure_draft_prefix(title)
        } else {
            title.to_string()
        };
        let description = compose_description(body, review_report);
        let url = format!("{}/projects/{id}/merge_requests", self.api_base);
        let payload = serde_json::json!({
            "source_branch": source_branch,
            "target_branch": base,
            "title": title,
            "description": description,
        });
        let resp = reqwest::Client::new()
            .post(&url)
            .header("PRIVATE-TOKEN", token)
            .header("User-Agent", "openspec-autocoder")
            .json(&payload)
            .send()
            .await
            .map_err(|e| anyhow!("gitlab MR POST failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let snippet = body_snippet(resp).await;
            return Err(anyhow!(
                "gitlab MR POST {owner}/{repo} returned {status}: {snippet}"
            ));
        }
        let mr: MrResponse = resp
            .json()
            .await
            .map_err(|e| anyhow!("gitlab MR response decode failed: {e}"))?;
        Ok(CreatedPr {
            html_url: mr.web_url,
            number: mr.iid,
        })
    }

    async fn list_open_prs(
        &self,
        owner: &str,
        repo: &str,
        head: &str,
        base: &str,
        token: &str,
    ) -> Result<Vec<OpenPr>> {
        let id = project_id(owner, repo);
        let source_branch = strip_head_owner(head);
        let url = format!("{}/projects/{id}/merge_requests", self.api_base);
        let resp = reqwest::Client::new()
            .get(&url)
            .query(&[
                ("state", "opened"),
                ("source_branch", source_branch),
                ("target_branch", base),
            ])
            .header("PRIVATE-TOKEN", token)
            .header("User-Agent", "openspec-autocoder")
            .send()
            .await
            .map_err(|e| anyhow!("gitlab MR list GET failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let snippet = body_snippet(resp).await;
            return Err(anyhow!(
                "gitlab MR list GET {owner}/{repo} returned {status}: {snippet}"
            ));
        }
        let items: Vec<MrResponse> = resp
            .json()
            .await
            .map_err(|e| anyhow!("gitlab MR list decode failed: {e}"))?;
        Ok(items
            .into_iter()
            .map(|mr| OpenPr {
                number: mr.iid,
                html_url: mr.web_url,
            })
            .collect())
    }

    async fn find_pr_by_head(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        _head_owner: &str,
        head_branch: &str,
    ) -> Result<Vec<PrSummary>> {
        let id = project_id(owner, repo);
        let url = format!("{}/projects/{id}/merge_requests", self.api_base);
        let resp = reqwest::Client::new()
            .get(&url)
            .query(&[("state", "opened"), ("source_branch", head_branch)])
            .header("PRIVATE-TOKEN", token)
            .header("User-Agent", "openspec-autocoder")
            .send()
            .await
            .map_err(|e| anyhow!("gitlab MR find GET failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let snippet = body_snippet(resp).await;
            return Err(anyhow!(
                "gitlab MR find GET {owner}/{repo} returned {status}: {snippet}"
            ));
        }
        let items: Vec<MrFull> = resp
            .json()
            .await
            .map_err(|e| anyhow!("gitlab MR find decode failed: {e}"))?;
        Ok(items.into_iter().map(MrFull::into_summary).collect())
    }

    async fn list_comments_since(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        pr_number: u64,
        since: DateTime<Utc>,
    ) -> Result<Vec<IssueComment>> {
        let id = project_id(owner, repo);
        let url = format!("{}/projects/{id}/merge_requests/{pr_number}/notes", self.api_base);
        let resp = reqwest::Client::new()
            .get(&url)
            .query(&[
                ("sort", "asc"),
                ("order_by", "created_at"),
                ("per_page", "100"),
            ])
            .header("PRIVATE-TOKEN", token)
            .header("User-Agent", "openspec-autocoder")
            .send()
            .await
            .map_err(|e| anyhow!("gitlab notes GET failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let snippet = body_snippet(resp).await;
            return Err(anyhow!(
                "gitlab notes GET {owner}/{repo}!{pr_number} returned {status}: {snippet}"
            ));
        }
        let notes: Vec<NoteItem> = resp
            .json()
            .await
            .map_err(|e| anyhow!("gitlab notes decode failed: {e}"))?;
        // System notes (e.g. "changed the description") are automated and
        // never carry a comment-sourced trigger; drop them. The strict
        // `created_at >= since` filter mirrors the GitHub `since` semantics.
        Ok(notes
            .into_iter()
            .filter(|n| !n.system && n.created_at >= since)
            .map(NoteItem::into_comment)
            .collect())
    }

    async fn post_comment(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        pr_number: u64,
        body: &str,
    ) -> Result<()> {
        self.post_note(token, owner, repo, pr_number, body).await
    }

    async fn post_review(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
        body: &str,
        decision: ReviewDecision,
        token: &str,
    ) -> Result<()> {
        match decision {
            // GitLab has no request-changes review state, so only an approve
            // verdict maps onto a review primitive (the MR approval). The
            // approval endpoint carries no body.
            ReviewDecision::Approve => {
                let id = project_id(owner, repo);
                let url = format!(
                    "{}/projects/{id}/merge_requests/{pr_number}/approve",
                    self.api_base
                );
                let resp = reqwest::Client::new()
                    .post(&url)
                    .header("PRIVATE-TOKEN", token)
                    .header("User-Agent", "openspec-autocoder")
                    .send()
                    .await
                    .map_err(|e| anyhow!("gitlab MR approve POST failed: {e}"))?;
                let status = resp.status();
                if !status.is_success() {
                    let snippet = body_snippet(resp).await;
                    return Err(anyhow!(
                        "gitlab MR approve POST {owner}/{repo}!{pr_number} returned {status}: {snippet}"
                    ));
                }
                Ok(())
            }
            // Request-changes AND comment both land as an MR note.
            ReviewDecision::RequestChanges | ReviewDecision::Comment => {
                self.post_note(token, owner, repo, pr_number, body).await
            }
        }
    }

    async fn create_fork(
        &self,
        upstream_owner: &str,
        upstream_repo: &str,
        token: &str,
    ) -> Result<()> {
        let id = project_id(upstream_owner, upstream_repo);
        let url = format!("{}/projects/{id}/fork", self.api_base);
        let resp = reqwest::Client::new()
            .post(&url)
            .header("PRIVATE-TOKEN", token)
            .header("User-Agent", "openspec-autocoder")
            .send()
            .await
            .map_err(|e| anyhow!("gitlab fork POST failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let snippet = body_snippet(resp).await;
            return Err(anyhow!(
                "gitlab fork POST {upstream_owner}/{upstream_repo} returned {status}: {snippet}"
            ));
        }
        Ok(())
    }

    async fn list_open_issues(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
    ) -> Result<Vec<ForgeIssue>> {
        // GitLab issues are a distinct endpoint from merge requests, so (unlike
        // GitHub) there are no MR entries to filter out. `iid` is the per-
        // project issue number; GitLab has no `author_association`.
        #[derive(Deserialize)]
        struct GlIssue {
            iid: u64,
            #[serde(default)]
            title: String,
            #[serde(default)]
            description: Option<String>,
            #[serde(default)]
            web_url: String,
        }
        let id = project_id(owner, repo);
        let url = format!("{}/projects/{id}/issues", self.api_base);
        let mut out: Vec<ForgeIssue> = Vec::new();
        let mut page: u32 = 1;
        loop {
            let page_s = page.to_string();
            let resp = reqwest::Client::new()
                .get(&url)
                .query(&[("state", "opened"), ("per_page", "100"), ("page", page_s.as_str())])
                .header("PRIVATE-TOKEN", token)
                .header("User-Agent", "openspec-autocoder")
                .send()
                .await
                .map_err(|e| anyhow!("gitlab issue list GET failed: {e}"))?;
            let status = resp.status();
            if !status.is_success() {
                let snippet = body_snippet(resp).await;
                return Err(anyhow!(
                    "gitlab issue list GET {owner}/{repo} returned {status}: {snippet}"
                ));
            }
            let items: Vec<GlIssue> = resp
                .json()
                .await
                .map_err(|e| anyhow!("gitlab issue list decode failed: {e}"))?;
            let count = items.len();
            for i in items {
                out.push(ForgeIssue {
                    number: i.iid,
                    title: i.title,
                    body: i.description.unwrap_or_default(),
                    author_association: None,
                    url: i.web_url,
                });
            }
            if count < 100 || page >= 50 {
                break;
            }
            page += 1;
        }
        Ok(out)
    }

    fn authorize(&self, comment: &IssueComment, auth: &CommandAuthorizationConfig) -> AuthLevel {
        // The synchronous, comment-local gate: an explicit `allowed_users`
        // login match (case-insensitive) authorizes without an API call. The
        // GitLab access-level gate (Developer+) is the async
        // [`GitlabForge::authorize_member`]; `author_association` is a
        // GitHub-only concept AND is not consulted here.
        let login = comment.user_login();
        if !login.is_empty()
            && auth
                .allowed_users
                .iter()
                .any(|u| u.eq_ignore_ascii_case(login))
        {
            return AuthLevel::Authorized;
        }
        AuthLevel::Unauthorized
    }

    fn branch_url(&self, owner: &str, repo: &str, branch: &str) -> String {
        // The GitLab MR-create hint for the push-only path: the "new merge
        // request from this source branch" web URL.
        format!(
            "{}/{owner}/{repo}/-/merge_requests/new?merge_request%5Bsource_branch%5D={branch}",
            self.web_base
        )
    }
}

impl GitlabForge {
    /// Toggle a merge request's draft state by adding/removing the GitLab
    /// `Draft:` title prefix (GitLab marks an MR draft via that prefix, not a
    /// flag). Reads the current title, recomputes the prefix, AND only issues
    /// the `PUT` when the title actually changes. `#[allow(dead_code)]`:
    /// exercised by tests AND wired by the Phase-3 GitLab daily-loop draft
    /// handling.
    #[allow(dead_code)]
    pub async fn set_pr_draft(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
        draft: bool,
        token: &str,
    ) -> Result<()> {
        let id = project_id(owner, repo);
        let base = format!("{}/projects/{id}/merge_requests/{pr_number}", self.api_base);
        let client = reqwest::Client::new();
        let resp = client
            .get(&base)
            .header("PRIVATE-TOKEN", token)
            .header("User-Agent", "openspec-autocoder")
            .send()
            .await
            .map_err(|e| anyhow!("gitlab MR GET failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let snippet = body_snippet(resp).await;
            return Err(anyhow!(
                "gitlab MR GET {owner}/{repo}!{pr_number} returned {status}: {snippet}"
            ));
        }
        #[derive(Deserialize)]
        struct MrTitle {
            title: String,
        }
        let mr: MrTitle = resp
            .json()
            .await
            .map_err(|e| anyhow!("gitlab MR title decode failed: {e}"))?;
        let new_title = if draft {
            ensure_draft_prefix(&mr.title)
        } else {
            strip_draft_prefix(&mr.title)
        };
        if new_title == mr.title {
            return Ok(());
        }
        let payload = serde_json::json!({ "title": new_title });
        let resp = client
            .put(&base)
            .header("PRIVATE-TOKEN", token)
            .header("User-Agent", "openspec-autocoder")
            .json(&payload)
            .send()
            .await
            .map_err(|e| anyhow!("gitlab MR PUT failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let snippet = body_snippet(resp).await;
            return Err(anyhow!(
                "gitlab MR PUT {owner}/{repo}!{pr_number} returned {status}: {snippet}"
            ));
        }
        Ok(())
    }

    /// Shared MR-note POST used by both `post_comment` AND the comment/
    /// request-changes arm of `post_review`.
    async fn post_note(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        pr_number: u64,
        body: &str,
    ) -> Result<()> {
        let id = project_id(owner, repo);
        let url = format!("{}/projects/{id}/merge_requests/{pr_number}/notes", self.api_base);
        let payload = serde_json::json!({ "body": body });
        let resp = reqwest::Client::new()
            .post(&url)
            .header("PRIVATE-TOKEN", token)
            .header("User-Agent", "openspec-autocoder")
            .json(&payload)
            .send()
            .await
            .map_err(|e| anyhow!("gitlab note POST failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let snippet = body_snippet(resp).await;
            return Err(anyhow!(
                "gitlab note POST {owner}/{repo}!{pr_number} returned {status}: {snippet}"
            ));
        }
        Ok(())
    }
}

/// One MR as returned by the create / list endpoints — only the fields
/// autocoder consults.
#[derive(Deserialize)]
struct MrResponse {
    iid: u64,
    web_url: String,
}

/// The fuller MR shape consulted by `find_pr_by_head`, mapped onto the
/// GitHub-shaped [`PrSummary`] the revision dispatcher reads.
#[derive(Deserialize)]
struct MrFull {
    iid: u64,
    title: String,
    web_url: String,
    state: String,
    #[serde(default)]
    description: Option<String>,
    created_at: DateTime<Utc>,
    source_branch: String,
    target_branch: String,
}

impl MrFull {
    fn into_summary(self) -> PrSummary {
        PrSummary {
            number: self.iid,
            title: self.title,
            url: self.web_url,
            state: self.state,
            body: self.description,
            created_at: self.created_at,
            head: PrRefSummary {
                ref_: self.source_branch,
            },
            base: PrRefSummary {
                ref_: self.target_branch,
            },
        }
    }
}

/// One MR note (GitLab's comment primitive).
#[derive(Deserialize)]
struct NoteItem {
    id: u64,
    body: String,
    #[serde(default)]
    author: Option<NoteAuthor>,
    created_at: DateTime<Utc>,
    #[serde(default)]
    system: bool,
}

#[derive(Deserialize)]
struct NoteAuthor {
    username: String,
}

impl NoteItem {
    fn into_comment(self) -> IssueComment {
        IssueComment {
            id: self.id,
            body: self.body,
            user: self.author.map(|a| IssueCommentUser { login: a.username }),
            created_at: self.created_at,
            // GitLab notes carry no `author_association`; the GitLab gate is
            // access-level based (see `authorize_member`).
            author_association: None,
        }
    }
}

/// Parse a GitLab repository URL into `(host, namespace, project)`. The
/// `namespace` may itself contain `/` for nested groups
/// (`group/subgroup`); `project` is the final path segment. Accepts the
/// SSH (`git@host:group/project.git`), HTTPS, AND `ssh://` forms, with or
/// without a trailing `.git`.
pub(crate) fn parse_gitlab_url(url: &str) -> Result<(String, String, String)> {
    let trimmed = url.trim();
    let without = trimmed.strip_suffix(".git").unwrap_or(trimmed);

    let (host, path) = if let Some(rest) = without.strip_prefix("git@") {
        let (host, path) = rest
            .split_once(':')
            .ok_or_else(|| unrecognized_gitlab_url(url))?;
        (host.to_string(), path.to_string())
    } else {
        let mut found = None;
        for scheme in ["https://", "http://", "ssh://git@", "ssh://"] {
            if let Some(rest) = without.strip_prefix(scheme) {
                let (host, path) = rest
                    .split_once('/')
                    .ok_or_else(|| unrecognized_gitlab_url(url))?;
                found = Some((host.to_string(), path.to_string()));
                break;
            }
        }
        found.ok_or_else(|| unrecognized_gitlab_url(url))?
    };

    let path = path.trim_matches('/');
    let (namespace, project) = path
        .rsplit_once('/')
        .ok_or_else(|| unrecognized_gitlab_url(url))?;
    if host.is_empty() || namespace.is_empty() || project.is_empty() {
        return Err(unrecognized_gitlab_url(url));
    }
    Ok((host.to_string(), namespace.to_string(), project.to_string()))
}

fn unrecognized_gitlab_url(url: &str) -> anyhow::Error {
    anyhow!(
        "unrecognized gitlab URL `{url}`: expected an SSH \
         (`git@host:namespace/project.git`) or HTTPS \
         (`https://host/namespace/project(.git)?`) form with a \
         `namespace/project` path"
    )
}

/// URL-encode the `namespace/project` path into the GitLab `:id` form
/// (`group%2Fsubgroup%2Fproject`). Encodes every byte outside the
/// unreserved set, so the path separators (`/`) become `%2F`.
pub(crate) fn project_id(owner: &str, repo: &str) -> String {
    encode_path(&format!("{owner}/{repo}"))
}

fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 8);
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Strip a fork-style `owner:branch` head qualifier down to the bare branch
/// name (GitLab's `source_branch`). Plain branch names pass through.
fn strip_head_owner(head: &str) -> &str {
    head.split_once(':').map(|(_, b)| b).unwrap_or(head)
}

/// The GitLab draft-title prefixes, lowercased, longest-first so a longer
/// marker is matched before a shorter one. GitLab accepts `Draft:` (current)
/// AND the legacy `WIP:`.
const DRAFT_PREFIXES: &[&str] = &["draft:", "wip:"];

/// `true` when `title` already starts with a GitLab draft marker
/// (case-insensitive).
fn has_draft_prefix(title: &str) -> bool {
    let lower = title.trim_start().to_ascii_lowercase();
    DRAFT_PREFIXES.iter().any(|p| lower.starts_with(p))
}

/// Add a `Draft: ` prefix unless one is already present.
fn ensure_draft_prefix(title: &str) -> String {
    if has_draft_prefix(title) {
        title.to_string()
    } else {
        format!("Draft: {title}")
    }
}

/// Remove a leading GitLab draft marker (`Draft:` / `WIP:`, case-insensitive)
/// AND any whitespace that followed it. `#[allow(dead_code)]`: reached only
/// via the (forward-compat) `set_pr_draft` disable path.
#[allow(dead_code)]
fn strip_draft_prefix(title: &str) -> String {
    let trimmed = title.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    for p in DRAFT_PREFIXES {
        if lower.starts_with(p) {
            return trimmed[p.len()..].trim_start().to_string();
        }
    }
    title.to_string()
}

/// Compose the MR description: the base body, plus a `## Code Review` section
/// (and the reviewer attribution line) when a report is present. Mirrors the
/// GitHub bundled-mode body shape.
fn compose_description(body: &str, review_report: Option<&ReviewReport>) -> String {
    let Some(report) = review_report else {
        return body.to_string();
    };
    let attr_suffix = report
        .attribution
        .as_deref()
        .map(|a| format!("\n\n{}", crate::attribution::attribution_line("Reviewer", a)))
        .unwrap_or_default();
    format!("{body}\n\n## Code Review\n\n{}{attr_suffix}", report.markdown)
}

/// Read up to 500 chars of a non-2xx response body for an error message.
async fn body_snippet(resp: reqwest::Response) -> String {
    resp.text()
        .await
        .map(|t| t.chars().take(500).collect::<String>())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_reviewer::{ReviewReport, ReviewVerdict};

    fn auth(users: &[&str]) -> CommandAuthorizationConfig {
        CommandAuthorizationConfig {
            allowed_associations: Vec::new(),
            allowed_users: users.iter().map(|s| s.to_string()).collect(),
            decline_comment: false,
        }
    }

    // ---- 4.6: parse_repo handles nested groups AND URL-encodes the path ----

    #[test]
    fn parse_repo_handles_nested_group_and_encodes_path() {
        let forge = GitlabForge::new("gitlab.example.com");
        for url in [
            "https://gitlab.example.com/group/subgroup/project.git",
            "git@gitlab.example.com:group/subgroup/project.git",
            "https://gitlab.example.com/group/subgroup/project",
        ] {
            let (owner, repo) = forge.parse_repo(url).expect("nested gitlab URL must parse");
            assert_eq!((owner.as_str(), repo.as_str()), ("group/subgroup", "project"), "{url}");
            assert_eq!(
                project_id(&owner, &repo),
                "group%2Fsubgroup%2Fproject",
                "{url}"
            );
        }
    }

    #[test]
    fn parse_repo_handles_single_level_path() {
        let forge = GitlabForge::new("gitlab.com");
        let (owner, repo) = forge
            .parse_repo("https://gitlab.com/owner/project.git")
            .unwrap();
        assert_eq!((owner.as_str(), repo.as_str()), ("owner", "project"));
        assert_eq!(project_id(&owner, &repo), "owner%2Fproject");
    }

    #[test]
    fn parse_gitlab_url_extracts_host() {
        let (host, ns, proj) =
            parse_gitlab_url("https://gitlab.example.com/group/sub/proj.git").unwrap();
        assert_eq!(host, "gitlab.example.com");
        assert_eq!(ns, "group/sub");
        assert_eq!(proj, "proj");
    }

    // ---- draft prefix helpers ----

    #[test]
    fn draft_prefix_add_and_strip_round_trip() {
        assert_eq!(ensure_draft_prefix("My MR"), "Draft: My MR");
        // Idempotent: don't double-prefix.
        assert_eq!(ensure_draft_prefix("Draft: My MR"), "Draft: My MR");
        assert_eq!(strip_draft_prefix("Draft: My MR"), "My MR");
        // Case-insensitive AND legacy WIP marker.
        assert_eq!(strip_draft_prefix("draft: My MR"), "My MR");
        assert_eq!(strip_draft_prefix("WIP: My MR"), "My MR");
        // No marker → unchanged.
        assert_eq!(strip_draft_prefix("My MR"), "My MR");
    }

    // ---- 4.5: authorize by access level ----

    #[test]
    fn authorize_access_level_developer_and_above() {
        // Owner (50), Maintainer (40), Developer (30) are authorized.
        for level in [50, 40, 30] {
            assert_eq!(
                GitlabForge::authorize_access_level(level),
                AuthLevel::Authorized,
                "level {level} must authorize"
            );
        }
        // Reporter (20) AND Guest (10) are denied.
        for level in [20, 10, 5, 0] {
            assert_eq!(
                GitlabForge::authorize_access_level(level),
                AuthLevel::Unauthorized,
                "level {level} must be denied"
            );
        }
    }

    #[tokio::test]
    async fn authorize_member_maps_access_level_via_api() {
        let mut server = mockito::Server::new_async().await;
        // Developer → authorized.
        let dev = server
            .mock("GET", "/projects/group%2Fproj/members/all/7")
            .match_header("private-token", "t")
            .with_status(200)
            .with_body(r#"{"id":7,"username":"dev","access_level":30}"#)
            .create_async()
            .await;
        let forge = GitlabForge::with_api_base(server.url());
        assert_eq!(
            forge.authorize_member("group", "proj", 7, "t").await.unwrap(),
            AuthLevel::Authorized
        );
        dev.assert_async().await;

        // Reporter → denied.
        let reporter = server
            .mock("GET", "/projects/group%2Fproj/members/all/8")
            .with_status(200)
            .with_body(r#"{"id":8,"username":"rep","access_level":20}"#)
            .create_async()
            .await;
        assert_eq!(
            forge.authorize_member("group", "proj", 8, "t").await.unwrap(),
            AuthLevel::Unauthorized
        );
        reporter.assert_async().await;

        // Not a member (404) → denied, not an error.
        let missing = server
            .mock("GET", "/projects/group%2Fproj/members/all/9")
            .with_status(404)
            .with_body(r#"{"message":"404 Not found"}"#)
            .create_async()
            .await;
        assert_eq!(
            forge.authorize_member("group", "proj", 9, "t").await.unwrap(),
            AuthLevel::Unauthorized
        );
        missing.assert_async().await;
    }

    #[test]
    fn authorize_trait_matches_allowed_user() {
        let forge = GitlabForge::new("gitlab.com");
        let a = auth(&["Trusted-User"]);
        let comment = IssueComment {
            id: 1,
            body: "@bot revise".into(),
            user: Some(IssueCommentUser {
                login: "trusted-user".into(),
            }),
            created_at: Utc::now(),
            author_association: None,
        };
        assert_eq!(forge.authorize(&comment, &a), AuthLevel::Authorized);
        let stranger = IssueComment {
            id: 2,
            body: "@bot revise".into(),
            user: Some(IssueCommentUser {
                login: "stranger".into(),
            }),
            created_at: Utc::now(),
            author_association: None,
        };
        assert_eq!(forge.authorize(&stranger, &a), AuthLevel::Unauthorized);
    }

    // ---- 1.6 / 3.1: branch_url is the GitLab MR-create hint ----

    #[test]
    fn branch_url_is_gitlab_mr_create_url() {
        let forge = GitlabForge::new("gitlab.example.com");
        assert_eq!(
            forge.branch_url("group/sub", "proj", "agent-q"),
            "https://gitlab.example.com/group/sub/proj/-/merge_requests/new?merge_request%5Bsource_branch%5D=agent-q"
        );
    }

    #[test]
    fn with_api_base_trims_api_v4_for_web_base() {
        let forge = GitlabForge::with_api_base("https://gitlab.example.com/api/v4");
        assert_eq!(
            forge.branch_url("o", "r", "b"),
            "https://gitlab.example.com/o/r/-/merge_requests/new?merge_request%5Bsource_branch%5D=b"
        );
    }

    // ---- 4.2: MR lifecycle round-trips ----

    #[tokio::test]
    async fn open_pr_posts_merge_request() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/projects/group%2Fproj/merge_requests")
            .match_header("private-token", "t")
            .match_body(mockito::Matcher::JsonString(
                r#"{"source_branch":"agent-q","target_branch":"main","title":"My MR","description":"body"}"#
                    .to_string(),
            ))
            .with_status(201)
            .with_body(r#"{"iid":5,"web_url":"https://gitlab.example.com/group/proj/-/merge_requests/5"}"#)
            .create_async()
            .await;
        let forge = GitlabForge::with_api_base(server.url());
        let pr = forge
            .open_pr("group", "proj", "agent-q", "main", "My MR", "body", "t", None, false)
            .await
            .expect("MR create should succeed");
        assert_eq!(pr.number, 5);
        assert_eq!(
            pr.html_url,
            "https://gitlab.example.com/group/proj/-/merge_requests/5"
        );
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn open_pr_draft_prefixes_title() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/projects/group%2Fproj/merge_requests")
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"title":"Draft: My MR"}"#.to_string(),
            ))
            .with_status(201)
            .with_body(r#"{"iid":6,"web_url":"u"}"#)
            .create_async()
            .await;
        let forge = GitlabForge::with_api_base(server.url());
        forge
            .open_pr("group", "proj", "agent-q", "main", "My MR", "body", "t", None, true)
            .await
            .expect("draft MR create should succeed");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn list_open_prs_parses_iids() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", mockito::Matcher::Any)
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("state".into(), "opened".into()),
                mockito::Matcher::UrlEncoded("source_branch".into(), "agent-q".into()),
                mockito::Matcher::UrlEncoded("target_branch".into(), "main".into()),
            ]))
            .with_status(200)
            .with_body(r#"[{"iid":42,"web_url":"https://gitlab.example.com/group/proj/-/merge_requests/42"}]"#)
            .create_async()
            .await;
        let forge = GitlabForge::with_api_base(server.url());
        let prs = forge
            .list_open_prs("group", "proj", "agent-q", "main", "t")
            .await
            .expect("list should succeed");
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0].number, 42);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn find_pr_by_head_matches_source_branch() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", mockito::Matcher::Any)
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("state".into(), "opened".into()),
                mockito::Matcher::UrlEncoded("source_branch".into(), "agent-q".into()),
            ]))
            .with_status(200)
            .with_body(
                r#"[{"iid":7,"title":"x","web_url":"u","state":"opened","description":"d","created_at":"2026-01-01T00:00:00Z","source_branch":"agent-q","target_branch":"main"}]"#,
            )
            .create_async()
            .await;
        let forge = GitlabForge::with_api_base(server.url());
        let prs = forge
            .find_pr_by_head("t", "group", "proj", "group", "agent-q")
            .await
            .expect("find should succeed");
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0].number, 7);
        assert_eq!(prs[0].head.ref_, "agent-q");
        assert_eq!(prs[0].base.ref_, "main");
        mock.assert_async().await;
    }

    // ---- 4.3: set_pr_draft toggles the Draft: title prefix ----

    #[tokio::test]
    async fn set_pr_draft_adds_then_removes_prefix() {
        let mut server = mockito::Server::new_async().await;
        // Enable draft: GET returns a plain title; PUT must set `Draft: ...`.
        let get_plain = server
            .mock("GET", "/projects/group%2Fproj/merge_requests/5")
            .with_status(200)
            .with_body(r#"{"title":"My MR"}"#)
            .create_async()
            .await;
        let put_draft = server
            .mock("PUT", "/projects/group%2Fproj/merge_requests/5")
            .match_body(mockito::Matcher::JsonString(
                r#"{"title":"Draft: My MR"}"#.to_string(),
            ))
            .with_status(200)
            .with_body(r#"{"iid":5}"#)
            .create_async()
            .await;
        let forge = GitlabForge::with_api_base(server.url());
        forge
            .set_pr_draft("group", "proj", 5, true, "t")
            .await
            .expect("enable draft should succeed");
        get_plain.assert_async().await;
        put_draft.assert_async().await;

        // Disable draft: GET returns a `Draft:`-prefixed title; PUT strips it.
        let get_draft = server
            .mock("GET", "/projects/group%2Fproj/merge_requests/5")
            .with_status(200)
            .with_body(r#"{"title":"Draft: My MR"}"#)
            .create_async()
            .await;
        let put_plain = server
            .mock("PUT", "/projects/group%2Fproj/merge_requests/5")
            .match_body(mockito::Matcher::JsonString(
                r#"{"title":"My MR"}"#.to_string(),
            ))
            .with_status(200)
            .with_body(r#"{"iid":5}"#)
            .create_async()
            .await;
        forge
            .set_pr_draft("group", "proj", 5, false, "t")
            .await
            .expect("disable draft should succeed");
        get_draft.assert_async().await;
        put_plain.assert_async().await;
    }

    // ---- 4.4: post_review approve → approval; else → note ----

    #[tokio::test]
    async fn post_review_approve_hits_approval_endpoint() {
        let mut server = mockito::Server::new_async().await;
        let approve = server
            .mock("POST", "/projects/group%2Fproj/merge_requests/5/approve")
            .match_header("private-token", "t")
            .with_status(201)
            .with_body(r#"{"id":1}"#)
            .create_async()
            .await;
        // The note endpoint must NOT be hit on approve.
        let note = server
            .mock("POST", "/projects/group%2Fproj/merge_requests/5/notes")
            .expect(0)
            .create_async()
            .await;
        let forge = GitlabForge::with_api_base(server.url());
        forge
            .post_review("group", "proj", 5, "LGTM", ReviewDecision::Approve, "t")
            .await
            .expect("approve should succeed");
        approve.assert_async().await;
        note.assert_async().await;
    }

    #[tokio::test]
    async fn post_review_request_changes_posts_note() {
        let mut server = mockito::Server::new_async().await;
        let note = server
            .mock("POST", "/projects/group%2Fproj/merge_requests/5/notes")
            .match_body(mockito::Matcher::JsonString(
                r#"{"body":"please fix"}"#.to_string(),
            ))
            .with_status(201)
            .with_body(r#"{"id":2}"#)
            .create_async()
            .await;
        let approve = server
            .mock("POST", "/projects/group%2Fproj/merge_requests/5/approve")
            .expect(0)
            .create_async()
            .await;
        let forge = GitlabForge::with_api_base(server.url());
        forge
            .post_review(
                "group",
                "proj",
                5,
                "please fix",
                ReviewDecision::RequestChanges,
                "t",
            )
            .await
            .expect("request-changes note should succeed");
        note.assert_async().await;
        approve.assert_async().await;
    }

    // ---- comments round-trip ----

    #[tokio::test]
    async fn list_comments_since_filters_system_and_old_notes() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", mockito::Matcher::Any)
            .match_query(mockito::Matcher::UrlEncoded("sort".into(), "asc".into()))
            .with_status(200)
            .with_body(
                r#"[
                    {"id":1,"body":"old","author":{"username":"a"},"created_at":"2026-01-01T00:00:00Z","system":false},
                    {"id":2,"body":"system","author":{"username":"a"},"created_at":"2026-02-02T00:00:00Z","system":true},
                    {"id":3,"body":"@bot revise","author":{"username":"alice"},"created_at":"2026-02-02T00:00:00Z","system":false}
                ]"#,
            )
            .create_async()
            .await;
        let forge = GitlabForge::with_api_base(server.url());
        let since = "2026-02-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let comments = forge
            .list_comments_since("t", "group", "proj", 5, since)
            .await
            .expect("notes list should succeed");
        // Only note 3 survives: note 1 is too old, note 2 is a system note.
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].id, 3);
        assert_eq!(comments[0].user_login(), "alice");
        mock.assert_async().await;
    }

    #[test]
    fn compose_description_appends_review_section() {
        let report = ReviewReport {
            verdict: ReviewVerdict::Pass,
            markdown: "VERDICT".to_string(),
            concerns: Vec::new(),
            per_change_sections: Vec::new(),
            attribution: None,
        };
        let out = compose_description("base", Some(&report));
        assert_eq!(out, "base\n\n## Code Review\n\nVERDICT");
        assert_eq!(compose_description("base", None), "base");
    }
}
