//! Wraps `tests/install_verify_test.sh` as a cargo-test invocation so the
//! bash test gate for the check-only installer runs alongside the Rust test
//! suite. Asserts the bash script exits 0; if any sub-case fails, the harness
//! prints the failures on stderr AND we surface them in the panic message.
//!
//! Lives outside the unit-test tree because install-verify.sh is a top-level
//! repo artifact, not a crate-level module.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn install_verify_integration_suite_passes() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest
        .parent()
        .expect("autocoder/ is a child of the repo root");
    let script = repo_root.join("tests").join("install_verify_test.sh");
    assert!(
        script.exists(),
        "install-verify.sh test harness missing at {}",
        script.display()
    );

    let output = Command::new("bash")
        .arg(&script)
        .output()
        .expect("failed to spawn bash for install-verify.sh test harness");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "install-verify.sh test harness failed (exit={:?})\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status.code()
    );
}
