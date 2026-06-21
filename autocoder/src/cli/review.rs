//! `autocoder review <repo> <target>` — on-demand code review from the
//! command line (on-demand-code-review). Connects to the running daemon's
//! control socket and submits the `review_target` action, which runs the
//! agentic reviewer against the resolved PR / commit / file-set / described
//! area AND reports the verdict. The review is advisory + read-only: it
//! opens no revision and changes no code or marker.
//!
//! Unlike `autocoder audit run`, there is no daemon-absent standalone path:
//! the reviewer AND the repository workspace live in the daemon, so a missing
//! socket is a clear error rather than a fallback.

use crate::control_socket;
use anyhow::{Result, anyhow};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

pub async fn execute(repo: String, target: Vec<String>) -> Result<()> {
    let paths = crate::cli::resolve_paths_from_env()?;
    execute_at(&control_socket::socket_path(&paths), repo, target).await
}

pub async fn execute_at(socket: &Path, repo: String, target: Vec<String>) -> Result<()> {
    // Validate the target shape locally so an obvious mistake (`review repo`
    // with no target, `review repo pr abc`) fails fast with the usage hint —
    // the daemon validates identically, but a local check avoids a round trip.
    crate::code_reviewer::ReviewTargetSpec::parse(&target).map_err(|e| anyhow!("{e}"))?;

    let stream = match UnixStream::connect(socket).await {
        Ok(s) => s,
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Err(anyhow!(
                "no running daemon at {} — `review` runs the daemon's reviewer against its \
                 repository clone, so the daemon must be running",
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
    submit_review_target(stream, &repo, &target).await
}

async fn submit_review_target(stream: UnixStream, repo: &str, target: &[String]) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let request = serde_json::json!({
        "action": "review_target",
        "url": repo,
        "target": target,
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
    let ok = resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    if !ok {
        // A discarded (no-verdict) session comes back ok:false — surface the
        // failure (gatekeepers-fail-closed) rather than printing a clean pass.
        let err = resp
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("(no error message)");
        return Err(anyhow!("review failed: {err}"));
    }
    let verdict = resp.get("verdict").and_then(|v| v.as_str()).unwrap_or("?");
    let body = resp.get("body").and_then(|v| v.as_str()).unwrap_or("");
    let sessions = resp.get("sessions").and_then(|v| v.as_u64()).unwrap_or(1);
    let chunk_clause = if sessions > 1 {
        format!(" ({sessions} chunked sessions)")
    } else {
        String::new()
    };
    println!("Code review verdict: {verdict}{chunk_clause}");
    if !body.trim().is_empty() {
        println!("\n{}", body.trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;
    use tokio::net::UnixListener;

    /// One-shot fake daemon that responds with `response` to the first
    /// connection (mirrors `cli/audit.rs::tests::fake_server`).
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
    async fn ok_response_prints_verdict() {
        let (_dir, socket) = fake_server(
            r#"{"ok":true,"verdict":"Approve","body":"looks good","sessions":1,"chunks":["all"]}"#,
        )
        .await;
        let res = execute_at(
            &socket,
            "myrepo".to_string(),
            vec!["pr".to_string(), "7".to_string()],
        )
        .await;
        assert!(res.is_ok(), "expected Ok, got: {res:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn discarded_review_surfaces_failure_not_clean_pass() {
        // A no-verdict session comes back ok:false with the discard reason;
        // the CLI must surface it as an error, never print a clean pass.
        let (_dir, socket) = fake_server(
            r#"{"ok":false,"discarded":true,"error":"recorded no valid submit_review submission"}"#,
        )
        .await;
        let res = execute_at(
            &socket,
            "myrepo".to_string(),
            vec!["pr".to_string(), "7".to_string()],
        )
        .await;
        let err = res.expect_err("a discarded review must surface as an error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("no valid submit_review"),
            "error must surface the discard reason: {msg}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn missing_daemon_is_a_clear_error() {
        let dir = TempDir::new().unwrap();
        let socket = dir.path().join("nope.sock");
        let res = execute_at(
            &socket,
            "myrepo".to_string(),
            vec!["pr".to_string(), "7".to_string()],
        )
        .await;
        let err = res.expect_err("missing daemon must error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("no running daemon"),
            "error must explain the daemon is required: {msg}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn malformed_target_fails_fast_locally() {
        // `pr abc` is not a valid PR number; the local pre-check rejects it
        // before any socket connection.
        let dir = TempDir::new().unwrap();
        let socket = dir.path().join("nope.sock");
        let res = execute_at(
            &socket,
            "myrepo".to_string(),
            vec!["pr".to_string(), "abc".to_string()],
        )
        .await;
        let err = res.expect_err("malformed target must error");
        let msg = format!("{err:#}");
        assert!(msg.contains("not a valid PR number"), "msg: {msg}");
    }
}
