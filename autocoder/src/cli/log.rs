//! `autocoder log <repo> [<count>]` — list a managed repository's recent
//! base-branch commits (code-rollback-recovery's read-only `log` command).
//! Connects to the running daemon's control socket and submits the
//! `recent_commits_log` action, which reads the base branch's most recent
//! commits (newest-first) AND returns them. Modifies nothing.
//!
//! Like `review`, there is no daemon-absent standalone path: the repository
//! workspace lives in the daemon, so a missing socket is a clear error.

use crate::control_socket;
use anyhow::{Result, anyhow};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// Default page size when the operator omits `<count>`.
const DEFAULT_COUNT: usize = 20;

pub async fn execute(repo: String, count: Option<usize>) -> Result<()> {
    let paths = crate::cli::resolve_paths_from_env()?;
    execute_at(&control_socket::socket_path(&paths), repo, count).await
}

pub async fn execute_at(socket: &Path, repo: String, count: Option<usize>) -> Result<()> {
    let count = count.unwrap_or(DEFAULT_COUNT).max(1);
    let stream = match UnixStream::connect(socket).await {
        Ok(s) => s,
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Err(anyhow!(
                "no running daemon at {} — `log` reads the daemon's repository clone, so the \
                 daemon must be running",
                socket.display(),
            ));
        }
        Err(e) => {
            return Err(anyhow!(
                "could not connect to control socket {}: {e}",
                socket.display(),
            ));
        }
    };
    submit(stream, &repo, count).await
}

async fn submit(stream: UnixStream, repo: &str, count: usize) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let request = serde_json::json!({
        "action": "recent_commits_log",
        "url": repo,
        "count": count,
    });
    let mut payload = request.to_string();
    payload.push('\n');
    write_half
        .write_all(payload.as_bytes())
        .await
        .map_err(|e| anyhow!("writing to control socket: {e}"))?;
    write_half
        .shutdown()
        .await
        .map_err(|e| anyhow!("shutdown of control socket: {e}"))?;
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .map_err(|e| anyhow!("reading control-socket response: {e}"))?;
    if line.is_empty() {
        return Err(anyhow!("control socket closed without responding"));
    }
    let resp: serde_json::Value = serde_json::from_str(line.trim())
        .map_err(|e| anyhow!("parsing control-socket response: {e}\nraw: {line}"))?;
    if !resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        let err = resp
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("(no error message)");
        return Err(anyhow!("log failed: {err}"));
    }
    let base_branch = resp.get("base_branch").and_then(|v| v.as_str()).unwrap_or("?");
    let commits = resp
        .get("commits")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    println!("Recent commits on `{base_branch}` (newest first):");
    if commits.is_empty() {
        println!("  (none)");
    }
    for c in &commits {
        let sha = c.get("short_sha").and_then(|v| v.as_str()).unwrap_or("");
        let date = c.get("date").and_then(|v| v.as_str()).unwrap_or("");
        let subject = c.get("subject").and_then(|v| v.as_str()).unwrap_or("");
        println!("  {sha}  {date}  {subject}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;
    use tokio::net::UnixListener;

    async fn fake_server(response: &'static str) -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let socket = dir.path().join("control.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let response_owned = response.to_string();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);
            let mut buf = String::new();
            let _ = reader.read_line(&mut buf).await;
            let mut bytes = response_owned.into_bytes();
            if !bytes.ends_with(b"\n") {
                bytes.push(b'\n');
            }
            let _ = write_half.write_all(&bytes).await;
            let _ = write_half.shutdown().await;
        });
        (dir, socket)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ok_response_prints_commits() {
        let (_dir, socket) = fake_server(
            r#"{"ok":true,"base_branch":"main","commits":[{"short_sha":"abc1234","date":"2026-06-20","subject":"latest"}]}"#,
        )
        .await;
        let res = execute_at(&socket, "myrepo".to_string(), Some(5)).await;
        assert!(res.is_ok(), "expected Ok, got: {res:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn missing_daemon_is_a_clear_error() {
        let dir = TempDir::new().unwrap();
        let socket = dir.path().join("nope.sock");
        let res = execute_at(&socket, "myrepo".to_string(), None).await;
        let err = res.expect_err("missing daemon must error");
        assert!(format!("{err:#}").contains("no running daemon"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn error_response_surfaces() {
        let (_dir, socket) =
            fake_server(r#"{"ok":false,"error":"`x` matched 2 repositories"}"#).await;
        let res = execute_at(&socket, "x".to_string(), None).await;
        let err = res.expect_err("ambiguous match must error");
        assert!(format!("{err:#}").contains("matched 2 repositories"));
    }
}
