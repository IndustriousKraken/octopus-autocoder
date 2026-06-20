//! `autocoder blame <repo> <path> <line>[-<line>] [--detect-moves]` —
//! provenance lookup (review-survival-provenance). Connects to the running
//! daemon's control socket AND submits the `provenance_lookup` action,
//! which runs `git blame` at `HEAD` for the requested line(s) AND reports,
//! per line, the introducing commit (short SHA, subject, date) plus the PR
//! when the commit subject names one (no fabricated PR). Read-only.
//!
//! Like `log` / `survives`, there is no daemon-absent standalone path: the
//! repository workspace lives in the daemon, so a missing socket is a
//! clear error.

use crate::chatops::operator_commands::parse_line_range;
use crate::control_socket;
use anyhow::{Result, anyhow};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

pub async fn execute(repo: String, path: String, line: String, detect_moves: bool) -> Result<()> {
    let paths = crate::cli::resolve_paths_from_env()?;
    execute_at(
        &control_socket::socket_path(&paths),
        repo,
        path,
        line,
        detect_moves,
    )
    .await
}

pub async fn execute_at(
    socket: &Path,
    repo: String,
    path: String,
    line: String,
    detect_moves: bool,
) -> Result<()> {
    let (start, end) = parse_line_range(&line).map_err(|e| anyhow!(e))?;
    let stream = match UnixStream::connect(socket).await {
        Ok(s) => s,
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Err(anyhow!(
                "no running daemon at {} — `blame` reads the daemon's repository clone, so the \
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
    submit(stream, &repo, &path, start, end, detect_moves).await
}

async fn submit(
    stream: UnixStream,
    repo: &str,
    path: &str,
    start: usize,
    end: usize,
    detect_moves: bool,
) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let request = serde_json::json!({
        "action": "provenance_lookup",
        "url": repo,
        "path": path,
        "start": start,
        "end": end,
        "detect_moves": detect_moves,
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
        return Err(anyhow!("blame failed: {err}"));
    }
    let report = resp
        .get("report")
        .and_then(|v| v.as_str())
        .unwrap_or("(no provenance report)");
    println!("{report}");
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
    async fn ok_response_prints_report() {
        let (_dir, socket) =
            fake_server(r#"{"ok":true,"report":"Provenance of `src/a.rs` at HEAD:\n  L1  `abc1234`  2026-01-01  add a (#5)  (PR #5)\n"}"#)
                .await;
        let res = execute_at(
            &socket,
            "myrepo".to_string(),
            "src/a.rs".to_string(),
            "1-3".to_string(),
            false,
        )
        .await;
        assert!(res.is_ok(), "expected Ok, got: {res:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn invalid_line_range_is_rejected_before_connecting() {
        let dir = TempDir::new().unwrap();
        let socket = dir.path().join("control.sock");
        let res = execute_at(
            &socket,
            "myrepo".to_string(),
            "src/a.rs".to_string(),
            "5-1".to_string(),
            false,
        )
        .await;
        let err = res.expect_err("inverted range must error");
        assert!(format!("{err:#}").contains("M <= N"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn missing_daemon_is_a_clear_error() {
        let dir = TempDir::new().unwrap();
        let socket = dir.path().join("nope.sock");
        let res = execute_at(
            &socket,
            "myrepo".to_string(),
            "src/a.rs".to_string(),
            "10".to_string(),
            false,
        )
        .await;
        let err = res.expect_err("missing daemon must error");
        assert!(format!("{err:#}").contains("no running daemon"));
    }
}
