//! `autocoder survives <repo> <pr N | commit SHA> [--detect-moves]` —
//! survival analysis (review-survival-provenance). Connects to the running
//! daemon's control socket AND submits the `survival_analysis` action,
//! which reports which of a past PR's OR commit's changes still survive
//! verbatim at `HEAD`. The report states its boundary plainly (verbatim,
//! not semantic — it under-reports, never over-reports). Read-only.
//!
//! Like `log` / `review`, there is no daemon-absent standalone path: the
//! repository workspace lives in the daemon, so a missing socket is a
//! clear error.

use crate::control_socket;
use anyhow::{Result, anyhow};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

pub async fn execute(repo: String, target: Vec<String>, detect_moves: bool) -> Result<()> {
    let paths = crate::cli::resolve_paths_from_env()?;
    execute_at(&control_socket::socket_path(&paths), repo, target, detect_moves).await
}

/// Parse the `pr <N>` / `commit <sha>` target tokens into a control-socket
/// request fragment. Returns `(field, json-value)` so the caller can set
/// either `"pr"` or `"commit"`.
fn parse_target(target: &[String]) -> Result<(&'static str, serde_json::Value)> {
    let kind = target
        .first()
        .map(|s| s.to_ascii_lowercase())
        .ok_or_else(|| anyhow!("missing target: expected `pr <N>` or `commit <SHA>`"))?;
    match kind.as_str() {
        "pr" => {
            let n: u64 = target
                .get(1)
                .ok_or_else(|| anyhow!("`pr` needs a number: `pr <N>`"))?
                .parse()
                .map_err(|_| anyhow!("`pr` needs a positive number"))?;
            if n < 1 {
                return Err(anyhow!("`pr` number must be >= 1"));
            }
            Ok(("pr", serde_json::json!(n)))
        }
        "commit" => {
            let sha = target
                .get(1)
                .ok_or_else(|| anyhow!("`commit` needs a SHA: `commit <SHA>`"))?;
            if !sha.chars().all(|c| c.is_ascii_hexdigit()) || sha.len() < 4 {
                return Err(anyhow!(
                    "that does not look like a commit SHA (expected >= 4 hex chars)"
                ));
            }
            Ok(("commit", serde_json::json!(sha)))
        }
        other => Err(anyhow!(
            "unknown target `{other}`: expected `pr <N>` or `commit <SHA>`"
        )),
    }
}

pub async fn execute_at(
    socket: &Path,
    repo: String,
    target: Vec<String>,
    detect_moves: bool,
) -> Result<()> {
    let (field, value) = parse_target(&target)?;
    let stream = match UnixStream::connect(socket).await {
        Ok(s) => s,
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Err(anyhow!(
                "no running daemon at {} — `survives` reads the daemon's repository clone, so the \
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
    submit(stream, &repo, field, value, detect_moves).await
}

async fn submit(
    stream: UnixStream,
    repo: &str,
    field: &str,
    value: serde_json::Value,
    detect_moves: bool,
) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut request = serde_json::json!({
        "action": "survival_analysis",
        "url": repo,
        "detect_moves": detect_moves,
    });
    request[field] = value;
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
        return Err(anyhow!("survives failed: {err}"));
    }
    let report = resp
        .get("report")
        .and_then(|v| v.as_str())
        .unwrap_or("(no survival report)");
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

    #[test]
    fn parse_target_recognizes_pr_and_commit() {
        let (f, v) = parse_target(&["pr".into(), "12".into()]).unwrap();
        assert_eq!(f, "pr");
        assert_eq!(v, serde_json::json!(12));
        let (f, v) = parse_target(&["commit".into(), "abcd1234".into()]).unwrap();
        assert_eq!(f, "commit");
        assert_eq!(v, serde_json::json!("abcd1234"));
        assert!(parse_target(&["pr".into(), "x".into()]).is_err());
        assert!(parse_target(&["commit".into(), "zz".into()]).is_err());
        assert!(parse_target(&["bogus".into()]).is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ok_response_prints_report() {
        let (_dir, socket) =
            fake_server(r#"{"ok":true,"report":"Survival of commit abc at HEAD\n","surviving_lines":3}"#)
                .await;
        let res = execute_at(
            &socket,
            "myrepo".to_string(),
            vec!["commit".into(), "abc1234".into()],
            false,
        )
        .await;
        assert!(res.is_ok(), "expected Ok, got: {res:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn missing_daemon_is_a_clear_error() {
        let dir = TempDir::new().unwrap();
        let socket = dir.path().join("nope.sock");
        let res = execute_at(
            &socket,
            "myrepo".to_string(),
            vec!["pr".into(), "1".into()],
            false,
        )
        .await;
        let err = res.expect_err("missing daemon must error");
        assert!(format!("{err:#}").contains("no running daemon"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn error_response_surfaces() {
        let (_dir, socket) = fake_server(r#"{"ok":false,"error":"workspace does not exist"}"#).await;
        let res = execute_at(
            &socket,
            "x".to_string(),
            vec!["pr".into(), "1".into()],
            false,
        )
        .await;
        let err = res.expect_err("error response must surface");
        assert!(format!("{err:#}").contains("workspace does not exist"));
    }
}
