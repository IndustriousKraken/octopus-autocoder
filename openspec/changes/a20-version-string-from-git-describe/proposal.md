## Why

`a04` wired the daemon's startup notification to `env!("CARGO_PKG_VERSION")` — the version string from `Cargo.toml`. In practice `Cargo.toml`'s `version =` field rarely matches what's actually running:

- An operator running master (built from source) sees `🆙 autocoder v0.1.0 started` even though their checkout is 23 commits past the most recent `v1.1.1` tag.
- Operators bumping Cargo.toml's version on every commit is absurd — autocoder ships 5-20 changes/day; that's 5-20 manual version bumps. Realistically operators bump Cargo.toml only at major/minor releases, leaving the patch-and-development-commit info invisible.
- The version line is supposed to help operators answer "what version is my deployment running RIGHT NOW?" The current behavior actively misleads them.

The Rust-idiomatic fix is `build.rs` running `git describe --tags --always --dirty` at compile time AND exposing the result as a build-time env var the binary reads. This is what most Rust projects do (cargo itself, rustup, ripgrep, fd, etc.):

- Build at a clean tag commit → `v1.1.1`
- Build 23 commits past a tag → `v1.1.1-23-g4abc123`
- Build with uncommitted local changes → `v1.1.1-23-g4abc123-dirty`
- Build from a source tarball with no `.git/` → Cargo.toml version verbatim (fallback)

Cargo.toml's `version =` becomes the "base version that operators manually bump at semver-meaningful releases" — semver discipline at release time, not per-commit. `git describe` provides the "delta past tag" info automatically.

## What Changes

**New `autocoder/build.rs`** that runs at every cargo build. Logic:

```rust
fn main() {
    let describe = std::process::Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    println!("cargo:rustc-env=AUTOCODER_VERSION={describe}");

    // Re-run when HEAD changes (dev builds reflect the working commit)
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-changed=.git/refs/tags");
}
```

The fallback chain: `git describe` failure → empty string fallback → Cargo.toml version. The result is ALWAYS a non-empty string, even on tarball builds with no `.git/` directory AND no git binary installed.

**Replace `env!("CARGO_PKG_VERSION")` with `env!("AUTOCODER_VERSION")` everywhere it appears in the daemon code.** Grep'd hits so far:
- The startup version notification (from `a04`'s implementation in `autocoder/src/cli/run.rs` or wherever bring-up lives)
- Clap's auto-generated `--version` output (override via `#[command(version = env!("AUTOCODER_VERSION"))]` on the top-level Cli struct)
- Any other place that surfaces version to operators (PR body footer? log lines at startup?)

**The `🆙` startup notification's format is unchanged in shape**, just gets the truthful version string:

```
🆙 autocoder v1.1.1-23-g4abc123 started — 8 repository(ies) configured
```

vs the pre-spec misleading:

```
🆙 autocoder v0.1.0 started — 8 repository(ies) configured
```

**The binary-release path stays clean.** The release workflow (also from `a04`) builds at the tagged commit (e.g. `v1.2.0`). `git describe` at that commit returns the clean tag (`v1.2.0`) with no `-N-gSHA` suffix. Operators installing via `update.sh` always see clean semver versions; only source-builders see the `-N-gSHA` development describe output.

**Source-tarball builds are non-regressive.** When `.git/` is absent (`cargo install autocoder` from crates.io, an unpacked tarball, etc.), the fallback to `env!("CARGO_PKG_VERSION")` produces the Cargo.toml version — same as today's behavior. No worse than the pre-spec UX.

## Impact

- **Affected specs:**
  - `orchestrator-cli` — one MODIFIED requirement: `Daemon emits a startup version notification on every successful boot`. Preserves all 4 existing scenarios; replaces `env!("CARGO_PKG_VERSION")` references with `env!("AUTOCODER_VERSION")`.
  - `orchestrator-cli` — one ADDED requirement: `Binary version string is derived from \`git describe\` at build time`. Defines build.rs, the fallback chain, AND the contract for what `env!("AUTOCODER_VERSION")` returns.
  - `project-documentation` — one ADDED requirement: `DEPLOYMENT.md AND CHATOPS.md explain the version-string format AND the source-vs-binary distinction`.
- **Affected code:**
  - `autocoder/build.rs` (new). The script above. Roughly 25 lines including comments.
  - `autocoder/Cargo.toml` — the `[package]` section's `build = "build.rs"` line (cargo auto-detects `build.rs` at crate root so this MAY be unnecessary; verify against current cargo behavior).
  - `autocoder/src/cli/mod.rs` (or wherever the top-level `Cli` struct lives) — extend clap derive:
    ```rust
    #[derive(Parser)]
    #[command(name = "autocoder", version = env!("AUTOCODER_VERSION"), about = "...")]
    pub struct Cli { ... }
    ```
    Override clap's default `version` (which uses Cargo.toml).
  - Every other reference to `env!("CARGO_PKG_VERSION")` swapped to `env!("AUTOCODER_VERSION")`. Grep target:
    ```bash
    grep -rn 'CARGO_PKG_VERSION' autocoder/src/
    ```
  - `docs/DEPLOYMENT.md` — add a "Version-string format" section under the existing upgrade discussion.
  - `docs/CHATOPS.md` — extend the `🆙` startup notification example to show the new format with the `-N-gSHA` suffix in dev-build cases.
- **Operator-visible behavior:**
  - The `🆙 autocoder v... started` notification now shows the truthful version. Operators running master see `v1.1.1-23-g4abc123` (or similar) instead of `v0.1.0`.
  - `autocoder --version` returns the same string instead of the Cargo.toml version.
  - Binary-release operators (installing via `update.sh`) see clean `vX.Y.Z` strings because the release workflow builds at tagged commits.
  - Cargo.toml's `version =` field becomes a "manually-bumped base version" — operators bump it at semver-meaningful releases (major/minor/patch) AND let `git describe` provide the delta info automatically.
- **Breaking:** no for end users. Cargo.toml's version field still exists AND still serves as the fallback. The `🆙` notification's format is unchanged; only the version string within it improves.
- **Acceptance:** `cargo test` passes (build.rs doesn't break the build); `cargo build --release` produces a binary whose `--version` output is non-empty AND not literally `0.1.0` (when run from a git checkout past v0.1.0); `openspec validate a20-version-string-from-git-describe --strict` passes. New unit test in `build.rs` may be impractical (`build.rs` runs at build time AND has limited testability); instead, an integration test in the test suite reads `env!("AUTOCODER_VERSION")` AND asserts the string is non-empty.
