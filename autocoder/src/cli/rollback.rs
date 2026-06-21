//! `autocoder rollback <repo> [--count N | --to SHA] [--confirm]` —
//! code-rollback recovery (code-rollback-recovery). Rolls a repository's CODE
//! back by a commit count OR to a target SHA WHILE unarchiving the
//! changes/issues archived in the rolled-back range (returning them to the
//! active lanes to be re-gated AND re-implemented). The untrusted code is
//! discarded; the sound spec/issue work re-enters the pipeline. The
//! operation rides the normal push + PR flow (honoring `auto_submit_pr`).
//!
//! DRY-RUN IS THE DEFAULT: without `--confirm`, the command reports exactly
//! what WOULD be rolled back AND unarchived, changing nothing. With
//! `--confirm`, it prints the preview, prompts for explicit confirmation,
//! AND — on `y` — performs the rollback. Like `review`/`log`, the workspace
//! lives in the daemon, so a missing socket is a clear error.

use crate::control_socket;
use anyhow::{Result, anyhow};
use std::io::{BufRead, Write};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// Arguments to the `rollback` subcommand collected from clap.
#[derive(Debug, Clone)]
pub struct RollbackArgs {
    pub repo: String,
    /// Roll back the last N commits.
    pub count: Option<usize>,
    /// Roll back to this commit SHA.
    pub to: Option<String>,
    /// Skip the dry-run-only default and actually perform the rollback
    /// (after a confirmation prompt).
    pub confirm: bool,
}

pub async fn execute(args: RollbackArgs) -> Result<()> {
    let paths = crate::cli::resolve_paths_from_env()?;
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    execute_with_io(
        &control_socket::socket_path(&paths),
        args,
        &mut stdin.lock(),
        &mut stdout.lock(),
    )
    .await
}

/// IO-injected core so tests can drive the confirmation prompt without a
/// real terminal.
pub async fn execute_with_io<R: BufRead, W: Write>(
    socket: &Path,
    args: RollbackArgs,
    reader: &mut R,
    writer: &mut W,
) -> Result<()> {
    // Validate depth shape locally so an obvious mistake fails fast.
    match (args.count, args.to.as_deref()) {
        (Some(_), Some(_)) => {
            return Err(anyhow!(
                "provide EITHER --count OR --to, not both"
            ));
        }
        (None, None) => {
            return Err(anyhow!(
                "missing rollback depth: pass --count <N> (last N commits) OR --to <SHA>"
            ));
        }
        (Some(0), None) => {
            return Err(anyhow!("--count must be at least 1"));
        }
        _ => {}
    }

    // Step 1: always run the dry-run preview first AND print it.
    let preview_resp = submit_rollback(socket, &args, true).await?;
    if !preview_resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        let err = preview_resp
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("(no error message)");
        return Err(anyhow!("rollback preview failed: {err}"));
    }
    let preview_text = preview_resp
        .get("preview")
        .and_then(|v| v.as_str())
        .unwrap_or("(no preview)");
    writeln!(writer, "{preview_text}")?;

    if !args.confirm {
        writeln!(
            writer,
            "\nDry run only. Re-run with --confirm to perform this rollback."
        )?;
        writer.flush()?;
        return Ok(());
    }

    let has_collisions = preview_resp
        .get("has_collisions")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if has_collisions {
        return Err(anyhow!(
            "rollback aborted: in-range unit(s) collide with existing active directories \
             (see the preview above). Resolve them, then retry."
        ));
    }

    // Step 2: confirmation prompt (mirrors `rewind`).
    write!(
        writer,
        "\nThis DISCARDS the code in the rolled-back range and opens a rollback PR (or pushes a \
         branch when auto_submit_pr is false). Proceed? [y/N] "
    )?;
    writer.flush()?;
    let mut buf = String::new();
    reader.read_line(&mut buf)?;
    let response = buf.trim();
    if response != "y" && response != "Y" {
        writeln!(writer, "rollback cancelled")?;
        writer.flush()?;
        return Ok(());
    }

    // Step 3: perform the rollback.
    let resp = submit_rollback(socket, &args, false).await?;
    if !resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        let err = resp
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("(no error message)");
        return Err(anyhow!("rollback failed: {err}"));
    }
    let outcome = resp.get("outcome").and_then(|v| v.as_str()).unwrap_or("");
    match outcome {
        "pr_opened" => {
            let pr_url = resp.get("pr_url").and_then(|v| v.as_str()).unwrap_or("?");
            writeln!(writer, "\n✓ Rollback PR opened: {pr_url}")?;
        }
        "branch_pushed_no_pr" => {
            let branch_url = resp.get("branch_url").and_then(|v| v.as_str()).unwrap_or("?");
            let suggested = resp
                .get("suggested_command")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            writeln!(
                writer,
                "\n✓ Rollback branch pushed (auto_submit_pr is false): {branch_url}\nRun: {suggested}"
            )?;
        }
        other => {
            writeln!(writer, "\n✓ Rollback complete (outcome: {other})")?;
        }
    }
    writer.flush()?;
    Ok(())
}

/// Connect, submit the `rollback_recovery` action (with `dry_run`), AND
/// return the parsed JSON response.
async fn submit_rollback(
    socket: &Path,
    args: &RollbackArgs,
    dry_run: bool,
) -> Result<serde_json::Value> {
    let stream = match UnixStream::connect(socket).await {
        Ok(s) => s,
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Err(anyhow!(
                "no running daemon at {} — `rollback` runs against the daemon's repository \
                 clone, so the daemon must be running",
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
    let mut request = serde_json::json!({
        "action": "rollback_recovery",
        "url": args.repo,
        "dry_run": dry_run,
    });
    if let Some(n) = args.count {
        request["count"] = serde_json::json!(n);
    }
    if let Some(sha) = &args.to {
        request["sha"] = serde_json::json!(sha);
    }
    let (read_half, mut write_half) = stream.into_split();
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
    let mut response_reader = BufReader::new(read_half);
    let mut line = String::new();
    response_reader
        .read_line(&mut line)
        .await
        .map_err(|e| anyhow!("reading control-socket response: {e}"))?;
    if line.is_empty() {
        return Err(anyhow!("control socket closed without responding"));
    }
    serde_json::from_str(line.trim())
        .map_err(|e| anyhow!("parsing control-socket response: {e}\nraw: {line}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::net::UnixListener;

    /// Multi-shot fake daemon: replies with the dry-run response to the
    /// first connection AND the act response to the second.
    async fn fake_server(dry: &'static str, act: &'static str) -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let socket = dir.path().join("control.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let responses = Arc::new([dry.to_string(), act.to_string()]);
        tokio::spawn(async move {
            for i in 0..2usize {
                let (stream, _) = match listener.accept().await {
                    Ok(s) => s,
                    Err(_) => return,
                };
                let (read_half, mut write_half) = stream.into_split();
                let mut reader = BufReader::new(read_half);
                let mut buf = String::new();
                let _ = reader.read_line(&mut buf).await;
                let mut bytes = responses[i].clone().into_bytes();
                if !bytes.ends_with(b"\n") {
                    bytes.push(b'\n');
                }
                let _ = write_half.write_all(&bytes).await;
                let _ = write_half.shutdown().await;
            }
        });
        (dir, socket)
    }

    fn args(repo: &str, count: Option<usize>, confirm: bool) -> RollbackArgs {
        RollbackArgs {
            repo: repo.to_string(),
            count,
            to: None,
            confirm,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn missing_depth_fails_fast() {
        let dir = TempDir::new().unwrap();
        let socket = dir.path().join("nope.sock");
        let mut input = std::io::Cursor::new(Vec::<u8>::new());
        let mut out = Vec::<u8>::new();
        let res = execute_with_io(&socket, args("r", None, false), &mut input, &mut out).await;
        let err = res.expect_err("missing depth must error");
        assert!(format!("{err:#}").contains("missing rollback depth"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn count_and_to_together_fails_fast() {
        let dir = TempDir::new().unwrap();
        let socket = dir.path().join("nope.sock");
        let mut input = std::io::Cursor::new(Vec::<u8>::new());
        let mut out = Vec::<u8>::new();
        let mut a = args("r", Some(2), false);
        a.to = Some("abc".to_string());
        let res = execute_with_io(&socket, a, &mut input, &mut out).await;
        let err = res.expect_err("count + to must error");
        assert!(format!("{err:#}").contains("EITHER --count OR --to"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dry_run_default_prints_preview_and_does_not_act() {
        let (_dir, socket) = fake_server(
            r#"{"ok":true,"dry_run":true,"preview":"WOULD roll back 2 commits","has_collisions":false}"#,
            r#"{"ok":false,"error":"the act endpoint must NOT be hit on a dry run"}"#,
        )
        .await;
        let mut input = std::io::Cursor::new(Vec::<u8>::new());
        let mut out = Vec::<u8>::new();
        // No --confirm: only the dry-run preview is fetched + printed.
        execute_with_io(&socket, args("r", Some(2), false), &mut input, &mut out)
            .await
            .expect("dry run succeeds");
        let printed = String::from_utf8(out).unwrap();
        assert!(printed.contains("WOULD roll back 2 commits"), "{printed}");
        assert!(printed.contains("Re-run with --confirm"), "{printed}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn confirm_declined_does_not_act() {
        let (_dir, socket) = fake_server(
            r#"{"ok":true,"dry_run":true,"preview":"WOULD roll back 1 commit","has_collisions":false}"#,
            r#"{"ok":true,"dry_run":false,"outcome":"pr_opened","pr_url":"http://x/pr/1"}"#,
        )
        .await;
        let mut input = std::io::Cursor::new(b"n\n".to_vec());
        let mut out = Vec::<u8>::new();
        execute_with_io(&socket, args("r", Some(1), true), &mut input, &mut out)
            .await
            .expect("decline returns Ok");
        let printed = String::from_utf8(out).unwrap();
        assert!(printed.contains("rollback cancelled"), "{printed}");
        // The PR url from the act response must NOT appear (we never acted).
        assert!(!printed.contains("http://x/pr/1"), "{printed}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn confirm_accepted_acts_and_reports_pr() {
        let (_dir, socket) = fake_server(
            r#"{"ok":true,"dry_run":true,"preview":"WOULD roll back 1 commit","has_collisions":false}"#,
            r#"{"ok":true,"dry_run":false,"outcome":"pr_opened","pr_url":"http://x/pr/42"}"#,
        )
        .await;
        let mut input = std::io::Cursor::new(b"y\n".to_vec());
        let mut out = Vec::<u8>::new();
        execute_with_io(&socket, args("r", Some(1), true), &mut input, &mut out)
            .await
            .expect("confirm acts");
        let printed = String::from_utf8(out).unwrap();
        assert!(printed.contains("Rollback PR opened: http://x/pr/42"), "{printed}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn confirm_with_collisions_aborts_before_prompt() {
        let (_dir, socket) = fake_server(
            r#"{"ok":true,"dry_run":true,"preview":"WOULD roll back 1 commit","has_collisions":true}"#,
            r#"{"ok":true,"dry_run":false,"outcome":"pr_opened","pr_url":"http://x/pr/1"}"#,
        )
        .await;
        // Even with --confirm and a `y` on stdin, a collision aborts.
        let mut input = std::io::Cursor::new(b"y\n".to_vec());
        let mut out = Vec::<u8>::new();
        let res =
            execute_with_io(&socket, args("r", Some(1), true), &mut input, &mut out).await;
        let err = res.expect_err("collision must abort");
        assert!(format!("{err:#}").contains("collide"), "{err:#}");
    }
}
