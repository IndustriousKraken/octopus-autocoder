//! GitHub REST API client for opening pull requests, plus URL parsing.

use anyhow::{Result, anyhow};
use serde::Deserialize;

const DEFAULT_API_BASE: &str = "https://api.github.com";

#[derive(Deserialize)]
struct PullResponse {
    html_url: String,
}

/// Open a pull request via the GitHub REST API. Returns the `html_url` of the
/// created PR on success.
pub async fn create_pull_request(
    owner: &str,
    repo: &str,
    head: &str,
    base: &str,
    title: &str,
    body: &str,
    token: &str,
) -> Result<String> {
    create_pull_request_at(DEFAULT_API_BASE, owner, repo, head, base, title, body, token).await
}

#[allow(clippy::too_many_arguments)]
async fn create_pull_request_at(
    api_base: &str,
    owner: &str,
    repo: &str,
    head: &str,
    base: &str,
    title: &str,
    body: &str,
    token: &str,
) -> Result<String> {
    let url = format!("{api_base}/repos/{owner}/{repo}/pulls");
    let payload = serde_json::json!({
        "title": title,
        "body": body,
        "head": head,
        "base": base,
    });

    let resp = reqwest::Client::new()
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "openspec-orchestrator")
        .json(&payload)
        .send()
        .await
        .map_err(|e| anyhow!("github pr request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        let truncated: String = text.chars().take(500).collect();
        return Err(anyhow!("github pr creation failed: {status}: {truncated}"));
    }

    let parsed: PullResponse = resp
        .json()
        .await
        .map_err(|e| anyhow!("github pr response decode failed: {e}"))?;
    Ok(parsed.html_url)
}

/// Parse a GitHub repository URL into `(owner, repo)`. Accepts both SSH and
/// HTTPS forms, with or without a trailing `.git`.
pub fn parse_repo_url(url: &str) -> Result<(String, String)> {
    let trimmed = url.trim();
    let without_suffix = trimmed.strip_suffix(".git").unwrap_or(trimmed);

    if let Some(rest) = without_suffix.strip_prefix("git@github.com:") {
        return split_owner_repo(rest, url);
    }
    if let Some(rest) = without_suffix
        .strip_prefix("https://github.com/")
        .or_else(|| without_suffix.strip_prefix("http://github.com/"))
        .or_else(|| without_suffix.strip_prefix("ssh://git@github.com/"))
    {
        return split_owner_repo(rest, url);
    }
    Err(anyhow!(
        "unrecognized github URL `{url}`: expected `git@github.com:<owner>/<repo>.git` or `https://github.com/<owner>/<repo>(.git)?`"
    ))
}

fn split_owner_repo(rest: &str, original: &str) -> Result<(String, String)> {
    let mut parts = rest.splitn(2, '/');
    let owner = parts.next().unwrap_or("");
    let repo = parts.next().unwrap_or("");
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return Err(anyhow!(
            "unrecognized github URL `{original}`: expected exactly `<owner>/<repo>`"
        ));
    }
    Ok((owner.to_string(), repo.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_url_ssh_with_git_suffix() {
        let (owner, repo) = parse_repo_url("git@github.com:owner/repo.git").unwrap();
        assert_eq!(owner, "owner");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn parse_url_ssh_without_git_suffix() {
        let (owner, repo) = parse_repo_url("git@github.com:owner/repo").unwrap();
        assert_eq!(owner, "owner");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn parse_url_https_with_git_suffix() {
        let (owner, repo) = parse_repo_url("https://github.com/owner/repo.git").unwrap();
        assert_eq!(owner, "owner");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn parse_url_https_without_git_suffix() {
        let (owner, repo) = parse_repo_url("https://github.com/owner/repo").unwrap();
        assert_eq!(owner, "owner");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn parse_url_ssh_url_form() {
        let (owner, repo) = parse_repo_url("ssh://git@github.com/owner/repo.git").unwrap();
        assert_eq!(owner, "owner");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn parse_url_rejects_non_github() {
        let err = parse_repo_url("https://gitlab.com/owner/repo.git")
            .expect_err("non-github URL should error");
        assert!(format!("{err:#}").contains("unrecognized"), "got: {err:#}");
    }

    #[test]
    fn parse_url_rejects_missing_repo_segment() {
        let err = parse_repo_url("git@github.com:owner").expect_err("missing repo should error");
        assert!(format!("{err:#}").contains("unrecognized"), "got: {err:#}");
    }

    #[test]
    fn parse_url_rejects_extra_path_segment() {
        let err = parse_repo_url("https://github.com/owner/repo/extra")
            .expect_err("extra path segment should error");
        assert!(format!("{err:#}").contains("unrecognized"), "got: {err:#}");
    }

    /// `mockito` smoke test: verify the request shape (path, headers, JSON
    /// body) and decoding of the `html_url` from a 201 response.
    #[tokio::test]
    async fn create_pull_request_posts_expected_request() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/repos/owner/repo/pulls")
            .match_header("authorization", "Bearer testtoken")
            .match_header("accept", "application/vnd.github+json")
            .match_header("user-agent", "openspec-orchestrator")
            .match_body(mockito::Matcher::JsonString(
                r#"{"title":"t","body":"b","head":"agent-q","base":"main"}"#.to_string(),
            ))
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_body(r#"{"html_url":"https://github.com/owner/repo/pull/1"}"#)
            .create_async()
            .await;

        let url = create_pull_request_at(
            &server.url(),
            "owner",
            "repo",
            "agent-q",
            "main",
            "t",
            "b",
            "testtoken",
        )
        .await
        .expect("PR creation should succeed");

        assert_eq!(url, "https://github.com/owner/repo/pull/1");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn create_pull_request_returns_err_on_non_2xx() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/repos/owner/repo/pulls")
            .with_status(422)
            .with_body(r#"{"message":"Validation Failed"}"#)
            .create_async()
            .await;

        let err = create_pull_request_at(
            &server.url(),
            "owner",
            "repo",
            "agent-q",
            "main",
            "t",
            "b",
            "testtoken",
        )
        .await
        .expect_err("422 should produce error");

        let msg = format!("{err:#}");
        assert!(msg.contains("422"), "expected 422 in error: {msg}");
        assert!(msg.contains("Validation Failed"), "expected body in error: {msg}");
    }
}
