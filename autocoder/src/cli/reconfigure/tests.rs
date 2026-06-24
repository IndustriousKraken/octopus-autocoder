//! Unit tests for the `--reconfigure` subsystem. Relocated from the former
//! inline `#[cfg(test)] mod tests` block in `install.rs` into this sibling
//! test module; `super` resolves to the `reconfigure` module root. The small
//! `config.yaml` fixtures (`baseline_answers`, `fixture_install_yaml`,
//! `write_fixture_config`, `loaded_probe`) are duplicated from `install.rs`'s
//! test module, whose copies still serve the wizard and `confirm_diff_and_apply`
//! tests that stayed behind.
use super::*;
use crate::cli::install::{
    ChatOpsBackendArg, InstallArgs, InstallMode, LoadState, RecordedCall, RecordingActions,
    ReviewerProviderArg, ScriptedIo, SystemdUnitProbe, WizardAnswers, assemble_config,
    execute_inner, serialize_config,
};
use crate::config::{Cadence, ChatOpsProvider, Config, ReviewerProvider};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn baseline_answers() -> WizardAnswers {
    WizardAnswers {
        repo_url: "git@github.com:acme/widgets.git".to_string(),
        base_branch: "main".to_string(),
        agent_branch: "agent-q".to_string(),
        poll_interval_sec: 300,
        token_env_var: "GITHUB_TOKEN".to_string(),
        github_pat: Some("ghp_test".to_string()),
        chatops_backend: ChatOpsBackendArg::None,
        chatops_channel_id: None,
        chatops_token: None,
        reviewer_provider: ReviewerProviderArg::None,
        reviewer_model: None,
        reviewer_api_key: None,
        reviewer_api_base_url: None,
        audits: HashMap::new(),
        canonical_rag: None,
        issues_enabled: false,
    }
}

/// Build a fixture `config.yaml` with audits/reviewer/chatops set so
/// reconfigure tests have realistic state to mutate.
fn fixture_install_yaml() -> String {
    // Assemble via the wizard's `assemble_config` so the YAML stays in
    // sync with whatever fields the bundled example carries.
    let ans = WizardAnswers {
        chatops_backend: ChatOpsBackendArg::Slack,
        chatops_channel_id: Some("C0123456789".to_string()),
        chatops_token: Some("xoxb-test".to_string()),
        reviewer_provider: ReviewerProviderArg::Anthropic,
        reviewer_model: Some("claude-sonnet-4-6".to_string()),
        reviewer_api_key: Some("sk-ant-test".to_string()),
        audits: {
            let mut m = HashMap::new();
            m.insert("drift_audit".to_string(), Cadence::Weekly);
            m
        },
        ..baseline_answers()
    };
    let cfg = assemble_config(&ans).expect("fixture assemble_config");
    serialize_config(&cfg).expect("fixture serialize_config")
}

fn write_fixture_config(tmp: &TempDir) -> PathBuf {
    let p = tmp.path().join("config.yaml");
    std::fs::write(&p, fixture_install_yaml()).unwrap();
    p
}

fn loaded_probe(config_path: &Path) -> SystemdUnitProbe {
    SystemdUnitProbe {
        load_state: LoadState::Loaded,
        fragment_path: Some(PathBuf::from("/etc/systemd/system/autocoder.service")),
        exec_start_config_path: Some(config_path.to_path_buf()),
    }
}

// --- 2.2 resolve_existing_config_path -----------------------------------

#[tokio::test]
async fn resolve_existing_config_path_dev_uses_home_default() {
    let tmp = TempDir::new().unwrap();
    let prior_home = std::env::var_os("HOME");
    let home = tmp.path().to_path_buf();
    let cfg_dir = home.join(".config/autocoder");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(cfg_dir.join("config.yaml"), b"# dev fixture\n").unwrap();
    unsafe {
        std::env::set_var("HOME", &home);
    }
    let args = InstallArgs::default();
    let actions = RecordingActions::new();
    let got = resolve_existing_config_path(&args, &actions, InstallMode::Dev)
        .await
        .unwrap();
    assert_eq!(got, cfg_dir.join("config.yaml"));
    unsafe {
        match prior_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
    // Dev mode must not invoke the systemd probe.
    for c in actions.calls() {
        assert!(
            !matches!(c, RecordedCall::ProbeSystemdUnit(_)),
            "dev mode must skip probe; saw {c:?}"
        );
    }
}

#[tokio::test]
async fn resolve_existing_config_path_server_probe_loaded_with_path() {
    let tmp = TempDir::new().unwrap();
    let unit_config = tmp.path().join("custom-config.yaml");
    std::fs::write(&unit_config, b"# unit config\n").unwrap();
    let actions = RecordingActions::new().with_probe_response(
        "autocoder.service",
        loaded_probe(&unit_config),
    );
    let args = InstallArgs::default();
    let got = resolve_existing_config_path(&args, &actions, InstallMode::Server)
        .await
        .unwrap();
    assert_eq!(got, unit_config);
}

#[tokio::test]
async fn resolve_existing_config_path_server_probe_not_found_no_default_bails() {
    let actions = RecordingActions::new(); // default probe: NotFound
    let args = InstallArgs::default();
    // /etc/autocoder/config.yaml almost certainly does not exist in
    // the sandbox. Assert the bail message rather than the path.
    let err = resolve_existing_config_path(&args, &actions, InstallMode::Server)
        .await
        .expect_err("expected bail when no probe + no default");
    assert!(
        format!("{err}").contains("no existing install detected"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn resolve_existing_config_path_honors_config_dir_override() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("config.yaml"), b"# override\n").unwrap();
    let actions = RecordingActions::new();
    let args = InstallArgs {
        config_dir: Some(tmp.path().to_path_buf()),
        ..InstallArgs::default()
    };
    let got = resolve_existing_config_path(&args, &actions, InstallMode::Server)
        .await
        .unwrap();
    assert_eq!(got, tmp.path().join("config.yaml"));
    // Override must short-circuit before the probe runs.
    for c in actions.calls() {
        assert!(
            !matches!(c, RecordedCall::ProbeSystemdUnit(_)),
            "config_dir override must skip probe; saw {c:?}"
        );
    }
}

#[tokio::test]
async fn resolve_existing_config_path_override_missing_bails() {
    let tmp = TempDir::new().unwrap();
    let actions = RecordingActions::new();
    let args = InstallArgs {
        config_dir: Some(tmp.path().to_path_buf()),
        ..InstallArgs::default()
    };
    let err = resolve_existing_config_path(&args, &actions, InstallMode::Server)
        .await
        .expect_err("expected bail when override path is empty");
    assert!(
        format!("{err}").contains("no existing install detected"),
        "unexpected error: {err}"
    );
}

// --- 4.4 per-section helpers --------------------------------------------

#[tokio::test]
async fn reconfigure_audits_updates_defaults_and_drops_disabled() {
    let raw = fixture_install_yaml();
    let existing: Config = serde_yml::from_str(&raw).unwrap();
    // LLM_DRIVEN_SLUGS order is: architecture_advisor, drift_audit,
    // missing_tests_audit, security_bug_audit, documentation_audit. The
    // wizard re-prompts each in order.
    let mut io = ScriptedIo::new(vec![
        "w", // architecture_advisor -> weekly
        "m", // drift_audit -> monthly (was weekly)
        "n", // missing_tests_audit -> never (drop)
        "d", // security_bug_audit -> daily
        "m", // documentation_audit -> monthly
    ]);
    let new_cfg = reconfigure_audits(&existing, &mut io).await.unwrap();
    let defaults = new_cfg
        .audits
        .as_ref()
        .expect("audits block present")
        .defaults
        .clone();
    assert_eq!(defaults.get("architecture_advisor"), Some(&Cadence::Weekly));
    assert_eq!(defaults.get("drift_audit"), Some(&Cadence::Monthly));
    assert!(!defaults.contains_key("missing_tests_audit"));
    assert_eq!(defaults.get("security_bug_audit"), Some(&Cadence::Daily));
    assert_eq!(defaults.get("documentation_audit"), Some(&Cadence::Monthly));
}

#[tokio::test]
async fn reconfigure_audits_all_never_drops_block() {
    let raw = fixture_install_yaml();
    let existing: Config = serde_yml::from_str(&raw).unwrap();
    let mut io = ScriptedIo::new(vec!["n", "n", "n", "n", "n"]);
    let new_cfg = reconfigure_audits(&existing, &mut io).await.unwrap();
    assert!(new_cfg.audits.is_none(), "no audits enabled → block omitted");
}

#[tokio::test]
async fn reconfigure_reviewer_switches_provider_and_model() {
    let raw = fixture_install_yaml();
    let existing: Config = serde_yml::from_str(&raw).unwrap();
    let mut io = ScriptedIo::new(vec![
        "3",                  // provider choice: openai_compatible (index 2 + 1)
        "grok-3",             // model
        "OPENAI_API_KEY",     // env var (bare-Enter would accept the existing default)
    ]);
    let new_cfg = reconfigure_reviewer(&existing, &mut io).await.unwrap();
    let r = new_cfg.reviewer.expect("reviewer block present");
    assert_eq!(r.provider, Some(ReviewerProvider::OpenAiCompatible));
    assert_eq!(r.model, "grok-3");
    assert_eq!(r.api_key_env.as_deref(), Some("OPENAI_API_KEY"));
}

#[tokio::test]
async fn reconfigure_reviewer_pick_none_clears_block() {
    let raw = fixture_install_yaml();
    let existing: Config = serde_yml::from_str(&raw).unwrap();
    let mut io = ScriptedIo::new(vec!["1"]); // "none"
    let new_cfg = reconfigure_reviewer(&existing, &mut io).await.unwrap();
    assert!(new_cfg.reviewer.is_none());
}

#[tokio::test]
async fn reconfigure_chatops_updates_channel_id() {
    let raw = fixture_install_yaml();
    let existing: Config = serde_yml::from_str(&raw).unwrap();
    let mut io = ScriptedIo::new(vec![
        "2",            // slack (idx 1 + 1)
        "C9999999999",  // new channel id
    ]);
    let new_cfg = reconfigure_chatops(&existing, &mut io).await.unwrap();
    let c = new_cfg.chatops.expect("chatops block present");
    assert_eq!(c.provider, ChatOpsProvider::Slack);
    assert_eq!(c.default_channel_id, "C9999999999");
    // Existing slack sub-block (with bot_token_env) preserved.
    let slack = c.slack.expect("slack sub-block preserved");
    assert_eq!(slack.bot_token_env.as_deref(), Some("SLACK_BOT_TOKEN"));
}

#[tokio::test]
async fn reconfigure_chatops_pick_none_drops_block() {
    let raw = fixture_install_yaml();
    let existing: Config = serde_yml::from_str(&raw).unwrap();
    let mut io = ScriptedIo::new(vec!["1"]); // "none"
    let new_cfg = reconfigure_chatops(&existing, &mut io).await.unwrap();
    assert!(new_cfg.chatops.is_none());
}

// --- 5.2 apply_in_place_patch -------------------------------------------

#[test]
fn apply_in_place_patch_updates_audits_subtree_only() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = write_fixture_config(&tmp);
    let raw_before = std::fs::read_to_string(&cfg_path).unwrap();
    let mut new_cfg: Config = serde_yml::from_str(&raw_before).unwrap();
    // Pre-condition: existing config carries drift_audit=weekly.
    {
        let defaults = new_cfg
            .audits
            .as_ref()
            .map(|a| a.defaults.clone())
            .unwrap_or_default();
        assert_eq!(defaults.get("drift_audit"), Some(&Cadence::Weekly));
    }
    // Mutate the audits subtree only.
    let mut audits = new_cfg.audits.clone().unwrap_or_default();
    audits
        .defaults
        .insert("drift_audit".to_string(), Cadence::Monthly);
    new_cfg.audits = Some(audits);

    apply_in_place_patch(&cfg_path, &new_cfg).unwrap();

    let raw_after = std::fs::read_to_string(&cfg_path).unwrap();
    let parsed: Config = serde_yml::from_str(&raw_after).unwrap();
    // Audits update landed.
    let defaults = parsed
        .audits
        .as_ref()
        .map(|a| a.defaults.clone())
        .unwrap_or_default();
    assert_eq!(defaults.get("drift_audit"), Some(&Cadence::Monthly));
    // Other top-level keys still parse to their prior values.
    let before_parsed: Config = serde_yml::from_str(&raw_before).unwrap();
    assert_eq!(parsed.github.token_env, before_parsed.github.token_env);
    assert_eq!(parsed.repositories.len(), before_parsed.repositories.len());
    assert_eq!(parsed.chatops.is_some(), before_parsed.chatops.is_some());
    assert_eq!(parsed.reviewer.is_some(), before_parsed.reviewer.is_some());
}

#[cfg(unix)]
#[test]
fn apply_in_place_patch_preserves_file_mode() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new().unwrap();
    let cfg_path = write_fixture_config(&tmp);
    std::fs::set_permissions(&cfg_path, std::fs::Permissions::from_mode(0o640)).unwrap();

    let raw_before = std::fs::read_to_string(&cfg_path).unwrap();
    let cfg: Config = serde_yml::from_str(&raw_before).unwrap();
    apply_in_place_patch(&cfg_path, &cfg).unwrap();

    let mode = std::fs::metadata(&cfg_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o640, "patch must preserve the pre-existing file mode");
}

// --- 3.2 / 3.3 execute_reconfigure integration --------------------------

fn dev_reconfigure_args(tmp: &TempDir, section: ReconfigureSection) -> InstallArgs {
    InstallArgs {
        mode: Some(InstallMode::Dev),
        config_dir: Some(tmp.path().to_path_buf()),
        reconfigure: Some(section),
        ..InstallArgs::default()
    }
}

#[tokio::test]
async fn execute_reconfigure_audits_in_place_patch_and_restart_guidance() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = write_fixture_config(&tmp);
    let mut io = ScriptedIo::new(vec![
        "w", // architecture_advisor
        "m", // drift_audit
        "n", // missing_tests_audit
        "d", // security_bug_audit
        "m", // documentation_audit
    ]);
    let actions = RecordingActions::new();
    let args = dev_reconfigure_args(&tmp, ReconfigureSection::Audits);
    execute_inner(args, &mut io, &actions, tmp.path().to_path_buf())
        .await
        .unwrap();
    let parsed: Config =
        serde_yml::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    let defaults = parsed
        .audits
        .as_ref()
        .map(|a| a.defaults.clone())
        .unwrap_or_default();
    assert_eq!(defaults.get("drift_audit"), Some(&Cadence::Monthly));
    assert_eq!(defaults.get("security_bug_audit"), Some(&Cadence::Daily));
}

#[tokio::test]
async fn execute_reconfigure_reviewer_decline_leaves_file_unchanged() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = write_fixture_config(&tmp);
    let raw_before = std::fs::read_to_string(&cfg_path).unwrap();
    // Provider -> openai_compatible, model -> grok-3, env var bare-Enter,
    // then decline at the diff prompt.
    let mut io = ScriptedIo::new(vec!["3", "grok-3", "", "n"]);
    let actions = RecordingActions::new();
    let args = dev_reconfigure_args(&tmp, ReconfigureSection::Reviewer);
    execute_inner(args, &mut io, &actions, tmp.path().to_path_buf())
        .await
        .unwrap();
    let raw_after = std::fs::read_to_string(&cfg_path).unwrap();
    assert_eq!(raw_before, raw_after, "decline must leave file unchanged");
    let out = io.output_str();
    assert!(out.contains("no changes made"), "expected `no changes made`:\n{out}");
}

#[tokio::test]
async fn execute_reconfigure_chatops_accept_applies_patch() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = write_fixture_config(&tmp);
    // slack -> slack, channel C9999999999, accept the diff.
    let mut io = ScriptedIo::new(vec!["2", "C9999999999", "y"]);
    let actions = RecordingActions::new();
    let args = dev_reconfigure_args(&tmp, ReconfigureSection::Chatops);
    execute_inner(args, &mut io, &actions, tmp.path().to_path_buf())
        .await
        .unwrap();
    let parsed: Config =
        serde_yml::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(
        parsed.chatops.as_ref().unwrap().default_channel_id,
        "C9999999999"
    );
}

#[tokio::test]
async fn execute_reconfigure_no_existing_install_bails() {
    let tmp = TempDir::new().unwrap();
    // No config.yaml under the override dir.
    let mut io = ScriptedIo::new(vec![]);
    let actions = RecordingActions::new();
    let args = dev_reconfigure_args(&tmp, ReconfigureSection::Audits);
    let err = execute_inner(args, &mut io, &actions, tmp.path().to_path_buf())
        .await
        .expect_err("expected bail with no install detected");
    let msg = format!("{err}");
    assert!(
        msg.contains("no existing install detected"),
        "unexpected error: {msg}"
    );
    assert!(
        msg.contains("install.sh"),
        "error should hint at install.sh: {msg}"
    );
}

#[tokio::test]
async fn execute_reconfigure_honors_probe_resolved_path_over_default() {
    // Server mode without a --config-dir override: the probe returns a
    // custom path that exists; the reconfigure flow must read AND write
    // there (not the /etc default).
    let tmp = TempDir::new().unwrap();
    let probe_cfg = tmp.path().join("probe-config.yaml");
    std::fs::write(&probe_cfg, fixture_install_yaml()).unwrap();
    let actions = RecordingActions::new().with_probe_response(
        "autocoder.service",
        loaded_probe(&probe_cfg),
    );
    let mut io = ScriptedIo::new(vec![
        "w", // architecture_advisor
        "m", // drift_audit
        "n", // missing_tests_audit
        "d", // security_bug_audit
        "m", // documentation_audit
    ]);
    let args = InstallArgs {
        mode: Some(InstallMode::Server),
        reconfigure: Some(ReconfigureSection::Audits),
        ..InstallArgs::default()
    };
    execute_inner(args, &mut io, &actions, tmp.path().to_path_buf())
        .await
        .unwrap();
    let parsed: Config =
        serde_yml::from_str(&std::fs::read_to_string(&probe_cfg).unwrap()).unwrap();
    let defaults = parsed
        .audits
        .as_ref()
        .map(|a| a.defaults.clone())
        .unwrap_or_default();
    assert_eq!(defaults.get("drift_audit"), Some(&Cadence::Monthly));
}
