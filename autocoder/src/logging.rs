//! Daemon logging sinks. The daemon writes its `tracing` event stream to TWO
//! sinks at the active (`RUST_LOG`) level: the existing stderr/journal sink
//! (unchanged) AND a rotated file under the logs directory (`<logs>/journal.*.log`),
//! so daemon-level diagnostics — including the predictable-failure categories
//! (`WorkspaceInitFailure`, `BranchPushFailure`, `PrCreationFailure`) and their
//! error chains — are greppable on disk alongside the per-session `runs/` logs.
//!
//! The logs directory is config-resolved and known only AFTER the global
//! subscriber is installed, so the file fmt layer uses a SWAPPABLE writer
//! ([`JournalMakeWriter`]): a no-op (`io::sink`) until [`attach_journal`] points
//! it at the rotated appender. Daily rotation + a bounded `max_log_files`
//! retention (operator-configurable) keep it from filling the disk.

use anyhow::{Context, Result};
use std::io;
use std::path::Path;
use std::sync::{OnceLock, RwLock};
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_subscriber::fmt::MakeWriter;

/// Default retained rotated segments (with daily rotation ≈ a week of history).
pub const DEFAULT_JOURNAL_MAX_FILES: usize = 7;

/// The live rotated-file writer, swapped in by [`attach_journal`] once the logs
/// directory is known. `None` → the file layer writes to `io::sink` (no-op).
static JOURNAL: RwLock<Option<NonBlocking>> = RwLock::new(None);
/// Keeps the non-blocking writer's flush worker alive for the process lifetime
/// (dropping the guard would stop draining the channel → lost logs).
static GUARD: OnceLock<WorkerGuard> = OnceLock::new();

/// `MakeWriter` for the file layer. Reads the current rotated appender (a cheap
/// `NonBlocking` clone) or falls back to `io::sink` before [`attach_journal`].
pub struct JournalMakeWriter;

impl<'a> MakeWriter<'a> for JournalMakeWriter {
    type Writer = Box<dyn io::Write + Send>;
    fn make_writer(&'a self) -> Self::Writer {
        match JOURNAL.read().ok().and_then(|g| g.clone()) {
            Some(nb) => Box::new(nb), // NonBlocking: io::Write + Send
            None => Box::new(io::sink()),
        }
    }
}

/// Install the global subscriber: the env-filter, the existing stderr sink, AND
/// the (initially no-op) rotated-file sink. Called once at startup.
pub fn init() {
    use tracing_subscriber::prelude::*;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(io::stderr))
        // The file sink: no ANSI escapes on disk; writer is swapped in later.
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(JournalMakeWriter),
        )
        .init();
}

/// Point the file sink at a daily-rotated `journal.*.log` under `logs_dir`,
/// retaining at most `max_files` segments. Idempotent-ish: a second call swaps
/// the appender (the first guard is retained, so its worker keeps draining).
pub fn attach_journal(logs_dir: &Path, max_files: usize) -> Result<()> {
    use tracing_appender::rolling::{Builder, Rotation};
    let appender = Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix("journal")
        .filename_suffix("log")
        .max_log_files(max_files.max(1))
        .build(logs_dir)
        .with_context(|| format!("building journal-log appender in {}", logs_dir.display()))?;
    let (nb, guard) = tracing_appender::non_blocking(appender);
    if let Ok(mut w) = JOURNAL.write() {
        *w = Some(nb);
    }
    let _ = GUARD.set(guard);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test (the global `JOURNAL`/`GUARD` statics are process-wide, so
    // parallel tests would race on them). Covers: the rotated appender writes a
    // greppable `journal.*.log` on disk; `attach_journal` wiring succeeds; and
    // the `max_files` floor keeps a `0` config from disabling retention.
    #[test]
    fn journal_appender_writes_rotated_file_and_attach_succeeds() {
        use std::io::Write;
        use tracing_appender::rolling::{Builder, Rotation};

        let dir = tempfile::tempdir().unwrap();
        // The synchronous appender `attach_journal` builds — deterministic to
        // assert (no non-blocking worker timing).
        let mut app = Builder::new()
            .rotation(Rotation::DAILY)
            .filename_prefix("journal")
            .filename_suffix("log")
            .max_log_files(7)
            .build(dir.path())
            .unwrap();
        writeln!(app, "BranchPushFailure: remote rejected push (greppable)").unwrap();
        app.flush().unwrap();

        let found = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let n = e.file_name();
                let n = n.to_string_lossy();
                n.starts_with("journal") && n.ends_with("log")
            })
            .any(|e| {
                std::fs::read_to_string(e.path())
                    .map(|c| c.contains("BranchPushFailure: remote rejected push"))
                    .unwrap_or(false)
            });
        assert!(found, "the logged line must appear in a journal.*.log segment");

        // Wiring: attach succeeds, and a `0` retention config is floored (never
        // panics / never disables pruning into unbounded growth).
        attach_journal(dir.path(), 7).unwrap();
        attach_journal(dir.path(), 0).unwrap();
        // The swappable writer constructs a usable sink once attached.
        let _w = JournalMakeWriter.make_writer();
    }
}
