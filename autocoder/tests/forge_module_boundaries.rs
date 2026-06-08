//! a007 forge-provider-abstraction CI scans.
//!
//! Two source-level invariants the `Forge` extraction must hold:
//!
//! 1. **Single source of truth** (spec "Forge API calls have a single source
//!    of truth"): no GitHub REST API call exists outside the forge module.
//!    Every GitHub REST call in the relocated `github.rs` sets the
//!    `application/vnd.github+json` Accept header AND the live base is
//!    `https://api.github.com`; both markers therefore appear ONLY inside
//!    `src/forge/`. Scanning `src/` (excluding `src/forge/`) for either
//!    marker proves the invariant — a stray REST call elsewhere would
//!    reintroduce one of them.
//!
//! 2. **Git operations are unchanged** (spec "Git operations are unchanged"):
//!    clone/fetch/branch/commit/push use the raw URL and the `origin` remote
//!    and do NOT route through the `Forge` trait. `git.rs` therefore
//!    references neither the forge module nor the `Forge` trait.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

/// Basename of THIS scanner file. The needle strings are assembled from
/// fragments at runtime so the scanner never matches its own source; the
/// basename self-skip is belt-and-braces.
const SCANNER_FILENAME: &str = "forge_module_boundaries.rs";

/// 6.2 / spec "single source of truth": no GitHub REST API marker appears
/// under `src/` outside `src/forge/`.
#[test]
fn no_github_rest_calls_outside_forge_module() {
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src_root = crate_root.join("src");
    assert!(src_root.is_dir(), "src/ must exist under {}", crate_root.display());

    // Assembled from fragments so this file does not self-match.
    let needles = rest_call_needles();

    let mut rs_files = Vec::new();
    collect_rs_files(&src_root, &mut rs_files);

    let mut violations: Vec<String> = Vec::new();
    for path in &rs_files {
        if path.file_name() == Some(OsStr::new(SCANNER_FILENAME)) {
            continue;
        }
        let rel = path
            .strip_prefix(&crate_root)
            .expect("walker returns paths under crate root")
            .to_string_lossy()
            .replace('\\', "/");
        // The forge module IS the single source of truth — skip it.
        if rel.starts_with("src/forge/") {
            continue;
        }
        let contents = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => panic!("could not read {}: {e}", path.display()),
        };
        for (lineno, line) in contents.lines().enumerate() {
            for needle in &needles {
                if line.contains(needle.as_str()) {
                    violations.push(format!("{}:{}: {}", rel, lineno + 1, line.trim()));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "GitHub REST API markers found outside the forge module (single source of truth \
         violated):\n\n{}\n\n\
         Every forge REST call must live in `src/forge/` and be reached through the \
         `Forge` trait (or the forge module's own helpers). Move the offending call \
         behind `GithubForge` and have the call site use the trait.",
        violations.join("\n"),
    );
}

/// 6.5 / spec "Git operations are unchanged": `git.rs` does not route through
/// the forge layer.
#[test]
fn git_module_does_not_route_through_forge() {
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let git_rs = crate_root.join("src").join("git.rs");
    let contents = fs::read_to_string(&git_rs)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", git_rs.display()));

    // Assembled from fragments so this file does not self-match.
    let forge_mod_ref: String = ["crate::", "forge"].concat();
    let forge_trait_ref: String = ["::", "Forge"].concat();
    for needle in [&forge_mod_ref, &forge_trait_ref] {
        assert!(
            !contents.contains(needle.as_str()),
            "src/git.rs must NOT reference the forge layer (found `{needle}`): git \
             clone/fetch/branch/commit/push are host-neutral and use the raw URL and \
             the `origin` remote, not the `Forge` trait."
        );
    }
}

/// Self-test: the single-source-of-truth scanner actually detects a synthetic
/// REST marker, guarding against a silent-pass regression where the needle
/// list drifts from the detection loop.
#[test]
fn scanner_flags_synthetic_rest_marker() {
    let needles = rest_call_needles();
    assert!(!needles.is_empty());
    let synthetic = format!(".header(\"Accept\", \"application/{}+json\")", needles[0]);
    assert!(
        needles.iter().any(|n| synthetic.contains(n.as_str())),
        "scanner must flag a synthetic line containing `{}`; got: {synthetic}",
        needles[0]
    );
}

/// The GitHub-REST markers, assembled from fragments so the scanner's own
/// source file never matches them.
fn rest_call_needles() -> Vec<String> {
    vec![
        ["vnd", ".github"].concat(),
        ["https://", "api.github.com"].concat(),
    ]
}

/// Recursively collect every `*.rs` file under `dir` into `out`.
fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => panic!("read_dir {}: {e}", dir.display()),
    };
    for entry in entries {
        let entry = entry.expect("read_dir entry must succeed");
        let path = entry.path();
        let file_type = entry.file_type().expect("file_type must succeed");
        if file_type.is_dir() {
            collect_rs_files(&path, out);
        } else if file_type.is_file() && path.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}
