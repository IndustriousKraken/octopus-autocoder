//! `autocoder install --reconfigure <section>` — re-prompt one section of an
//! existing install and patch the existing `config.yaml` in place.
//!
//! Extracted from [`crate::cli::install`] so the install wizard keeps a single
//! responsibility (fresh-install flow) while this module owns the
//! reconfigure-an-existing-install surface. The shared prompt helpers,
//! arg-enum mappers, and `serialize_config` live in `install` and are reused
//! here via `pub(crate)` paths; the CLI surface (`--reconfigure
//! <audits|reviewer|chatops>`, its allowlist, and all printed guidance) is
//! unchanged.
use crate::cli::install::{
    CHATOPS_OPTIONS, ChatOpsBackendArg, DEFAULT_SERVER_CONFIG_PATH, InstallArgs, InstallMode,
    LLM_DRIVEN_SLUGS, LoadState, REVIEWER_OPTIONS, ReviewerProviderArg, SystemActions, WizardIo,
    ask_audit_cadence, ask_default, audit_description, cadence_label, chatops_arg_to_idx,
    chatops_backend_label, idx_to_chatops_arg, idx_to_reviewer_arg, reviewer_arg_to_idx,
    reviewer_env_var, serialize_config,
};
use crate::config::{
    Cadence, ChatOpsConfig, ChatOpsProvider, Config, ReviewerConfig, ReviewerProvider,
    SlackProviderConfig,
};
use anyhow::{Context, Result, anyhow, bail};
use clap::ValueEnum;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Which slice of `config.yaml` the operator wants to re-prompt for via
/// `autocoder install --reconfigure <section>`. The wizard intentionally
/// excludes several knobs from this surface:
///
/// - `repositories`: add/remove flows hot-apply via `autocoder reload`; the
///   reconfigure verb deliberately does not grow into that space.
/// - `paths.*`: relocating the daemon data directories is a destructive
///   operation that needs explicit operator action AND a daemon restart.
/// - `executor.*`: every executor knob requires a restart; reconfigure
///   stays in the hot-applicable space.
/// - `audits.settings.*.prompt_path` and `audits.settings.*.extra.*`:
///   advanced per-audit overrides. The wizard handles only the
///   `audits.defaults.*` cadences; operators editing prompts or
///   thresholds edit YAML directly.
#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq)]
pub enum ReconfigureSection {
    Audits,
    Reviewer,
    Chatops,
}

const RECONFIGURE_NO_INSTALL_HINT: &str =
    "no existing install detected; run install.sh for first-time setup";

/// Resolve the path to the existing `config.yaml` for `--reconfigure`.
///
/// Resolution priority:
///
/// 1. `args.config_dir.join("config.yaml")` if the override exists.
/// 2. Server mode: probe `autocoder.service`; if loaded AND its
///    `exec_start_config_path` exists on disk, use it.
/// 3. Server mode: fall back to `/etc/autocoder/config.yaml`.
/// 4. Dev mode: `~/.config/autocoder/config.yaml`.
///
/// Bails with `RECONFIGURE_NO_INSTALL_HINT` if none of the above resolves
/// to a file that exists on disk.
pub(crate) async fn resolve_existing_config_path(
    args: &InstallArgs,
    actions: &dyn SystemActions,
    mode: InstallMode,
) -> Result<PathBuf> {
    if let Some(dir) = args.config_dir.as_ref() {
        let candidate = dir.join("config.yaml");
        if candidate.exists() {
            return Ok(candidate);
        }
        bail!(RECONFIGURE_NO_INSTALL_HINT);
    }

    match mode {
        InstallMode::Server => {
            let probe = actions.probe_systemd_unit("autocoder.service").await?;
            if matches!(probe.load_state, LoadState::Loaded)
                && let Some(p) = probe.exec_start_config_path.as_ref()
                && p.exists()
            {
                return Ok(p.clone());
            }
            let fallback = PathBuf::from(DEFAULT_SERVER_CONFIG_PATH);
            if fallback.exists() {
                return Ok(fallback);
            }
            bail!(RECONFIGURE_NO_INSTALL_HINT);
        }
        InstallMode::Dev => {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            let candidate = home.join(".config/autocoder/config.yaml");
            if candidate.exists() {
                return Ok(candidate);
            }
            bail!(RECONFIGURE_NO_INSTALL_HINT);
        }
    }
}

/// Per-section dispatcher invoked from `execute_inner` when
/// `args.reconfigure` is `Some`. Resolves the existing config path,
/// parses it, calls the section-specific re-prompt helper, and applies
/// the result (audits → in-place patch; reviewer / chatops → diff-confirm).
pub(crate) async fn execute_reconfigure(
    args: &InstallArgs,
    section: ReconfigureSection,
    io: &mut dyn WizardIo,
    actions: &dyn SystemActions,
    mode: InstallMode,
) -> Result<()> {
    let config_path = resolve_existing_config_path(args, actions, mode).await?;
    let raw = std::fs::read_to_string(&config_path)
        .with_context(|| format!("read existing config at {}", config_path.display()))?;
    let existing: Config = serde_yml::from_str(&raw)
        .with_context(|| format!("parse existing config at {}", config_path.display()))?;

    match section {
        ReconfigureSection::Audits => {
            let new_config = reconfigure_audits(&existing, io).await?;
            apply_in_place_patch(&config_path, &new_config)?;
            print_restart_guidance(&config_path, section);
        }
        ReconfigureSection::Reviewer => {
            let new_config = reconfigure_reviewer(&existing, io).await?;
            let applied =
                confirm_diff_and_apply(&config_path, &new_config, io).await?;
            if applied {
                print_restart_guidance(&config_path, section);
            } else {
                io.print("no changes made\n");
            }
        }
        ReconfigureSection::Chatops => {
            let new_config = reconfigure_chatops(&existing, io).await?;
            let applied =
                confirm_diff_and_apply(&config_path, &new_config, io).await?;
            if applied {
                print_restart_guidance(&config_path, section);
            } else {
                io.print("no changes made\n");
            }
        }
    }
    Ok(())
}

fn section_label(section: ReconfigureSection) -> &'static str {
    match section {
        ReconfigureSection::Audits => "audits.defaults.*",
        ReconfigureSection::Reviewer => "reviewer:",
        ReconfigureSection::Chatops => "chatops:",
    }
}

fn print_restart_guidance(config_path: &Path, section: ReconfigureSection) {
    println!(
        "Patched {} in {}.\nTo apply: sudo -u autocoder autocoder reload",
        section_label(section),
        config_path.display()
    );
}

/// Re-prompt the audit cadences with the operator's current values shown
/// as defaults, returning a clone of `existing` with the updated
/// `audits.defaults`. Decline (`never`) drops the slug from the map.
pub(crate) async fn reconfigure_audits(
    existing: &Config,
    io: &mut dyn WizardIo,
) -> Result<Config> {
    let current: HashMap<String, Cadence> = existing
        .audits
        .as_ref()
        .map(|a| a.defaults.clone())
        .unwrap_or_default();

    io.print("\nReconfigure audit cadences\n");
    io.print(
        "  Each prompt's default is the existing cadence. Pick `n` to disable.\n",
    );

    let mut updated: HashMap<String, Cadence> = HashMap::new();
    for (slug, rec) in LLM_DRIVEN_SLUGS {
        let existing_cadence = current
            .get(*slug)
            .copied()
            .unwrap_or(Cadence::Disabled);
        io.print(&format!("\n  {slug} ({})\n", audit_description(slug)));
        let label = format!("  Cadence (recommended: {})", cadence_label(*rec));
        let chosen = ask_audit_cadence(io, &label, existing_cadence, "never").await?;
        if chosen != Cadence::Disabled {
            updated.insert((*slug).to_string(), chosen);
        }
    }

    let mut new_config = existing.clone();
    if updated.is_empty() {
        new_config.audits = None;
    } else {
        let mut audits = existing.audits.clone().unwrap_or_default();
        audits.defaults = updated;
        new_config.audits = Some(audits);
    }
    Ok(new_config)
}

/// Re-prompt the reviewer block (provider + model + api-key env-var) with
/// existing values as defaults. Returns a clone of `existing` with the
/// updated `reviewer:` block.
pub(crate) async fn reconfigure_reviewer(
    existing: &Config,
    io: &mut dyn WizardIo,
) -> Result<Config> {
    let current_provider_arg = match existing.reviewer.as_ref().and_then(|r| r.provider) {
        Some(ReviewerProvider::Anthropic) => ReviewerProviderArg::Anthropic,
        Some(ReviewerProvider::OpenAiCompatible) => ReviewerProviderArg::OpenAiCompatible,
        Some(ReviewerProvider::Ollama) => ReviewerProviderArg::Ollama,
        // a69: the Google/Antigravity provider is agentic-only (driven by the
        // `agy` CLI) and is not surfaced in this oneshot-oriented wizard;
        // operators configure it by editing the reviewer block directly. On
        // reconfigure we present no pre-selection rather than a wrong one.
        Some(ReviewerProvider::Google) => ReviewerProviderArg::None,
        None => ReviewerProviderArg::None,
    };

    io.print("\nReconfigure reviewer\n");
    let idx = io
        .choose(
            "Reviewer provider",
            REVIEWER_OPTIONS,
            reviewer_arg_to_idx(current_provider_arg),
        )
        .await?;
    let provider_arg = idx_to_reviewer_arg(idx);

    let mut new_config = existing.clone();
    new_config.reviewer = match provider_arg {
        ReviewerProviderArg::None => None,
        ReviewerProviderArg::Anthropic
        | ReviewerProviderArg::OpenAiCompatible
        | ReviewerProviderArg::Ollama => {
            let provider = match provider_arg {
                ReviewerProviderArg::Anthropic => ReviewerProvider::Anthropic,
                ReviewerProviderArg::OpenAiCompatible => ReviewerProvider::OpenAiCompatible,
                ReviewerProviderArg::Ollama => ReviewerProvider::Ollama,
                _ => unreachable!(),
            };
            let default_model = existing
                .reviewer
                .as_ref()
                .map(|r| r.model.clone())
                .unwrap_or_else(|| match provider_arg {
                    ReviewerProviderArg::Anthropic => "claude-sonnet-4-6".to_string(),
                    ReviewerProviderArg::Ollama => "qwen2.5-coder:32b".to_string(),
                    _ => "gpt-4o-mini".to_string(),
                });
            let model = ask_default(io, "Reviewer model", &default_model).await?;
            // a37: Ollama branch — prompt for `api_base_url` (REQUIRED
            // by config-load) AND NO `api_key_env` (config-load REJECTS
            // a key for ollama).
            let (api_key_env, api_base_url) = if provider_arg == ReviewerProviderArg::Ollama
            {
                let existing_base = existing
                    .reviewer
                    .as_ref()
                    .and_then(|r| r.api_base_url.clone())
                    .unwrap_or_else(|| "http://localhost:11434".to_string());
                let base = ask_default(io, "Reviewer Ollama base URL", &existing_base).await?;
                (None, Some(base))
            } else {
                let default_env = existing
                    .reviewer
                    .as_ref()
                    .and_then(|r| r.api_key_env.clone())
                    .or_else(|| reviewer_env_var(provider_arg).map(String::from))
                    .unwrap_or_default();
                let api_key_env_raw = ask_default(io, "Reviewer API key env var", &default_env).await?;
                let key_env = if api_key_env_raw.is_empty() {
                    None
                } else {
                    Some(api_key_env_raw)
                };
                let existing_base = existing
                    .reviewer
                    .as_ref()
                    .and_then(|r| r.api_base_url.clone());
                (key_env, existing_base)
            };
            // Preserve all other reviewer fields (inline `api_key`,
            // `prompt_template_path`, etc.) from the existing config —
            // only provider/model/api_key_env/api_base_url are
            // reconfigured here.
            let mut reviewer = existing.reviewer.clone().unwrap_or_else(|| ReviewerConfig {
                enabled: true,
                provider: Some(provider),
                model: model.clone(),
                api_key_env: api_key_env.clone(),
                api_key: None,
                api_base_url: api_base_url.clone(),
                prompt_template_path: None,
                code_review: None,
                auto_revise: crate::config::AutoRevise::Off,
                prompt_budget_chars: 2_000_000,
                mode: crate::config::ReviewerMode::Bundled,
                max_code_reviews_per_pr: None,
                suggest_rereview_threshold: None,
                skip_spec_only_prs: false,
                // a64: track the spec-mandated `reviewer.kind` default
                // (now `Agentic`) rather than pinning the wizard's output to
                // `oneshot`. On a host whose reviewer CLI is missing, the
                // daemon's startup fallback degrades to `oneshot` with a WARN.
                kind: crate::config::ReviewerKind::default(),
                command: "claude".to_string(),
            });
            reviewer.provider = Some(provider);
            reviewer.model = model;
            reviewer.api_key_env = api_key_env;
            // For ollama, clear any pre-existing inline `api_key` (the
            // validator would reject it). For other providers, leave
            // inline `api_key` untouched (the operator may have set
            // it deliberately).
            if provider_arg == ReviewerProviderArg::Ollama {
                reviewer.api_key = None;
            }
            reviewer.api_base_url = api_base_url;
            Some(reviewer)
        }
    };
    Ok(new_config)
}

/// Re-prompt the chatops block (provider + default channel id) with
/// existing values as defaults. Returns a clone of `existing` with the
/// updated `chatops:` block (or absent, if the operator picks `none`).
pub(crate) async fn reconfigure_chatops(
    existing: &Config,
    io: &mut dyn WizardIo,
) -> Result<Config> {
    let current_backend_arg = match existing.chatops.as_ref().map(|c| c.provider) {
        Some(ChatOpsProvider::Slack) => ChatOpsBackendArg::Slack,
        Some(ChatOpsProvider::Discord) => ChatOpsBackendArg::Discord,
        Some(ChatOpsProvider::Teams) => ChatOpsBackendArg::Teams,
        Some(ChatOpsProvider::Mattermost) => ChatOpsBackendArg::Mattermost,
        Some(ChatOpsProvider::Matrix) => ChatOpsBackendArg::Matrix,
        None => ChatOpsBackendArg::None,
    };

    io.print("\nReconfigure chatops\n");
    let idx = io
        .choose(
            "ChatOps backend",
            CHATOPS_OPTIONS,
            chatops_arg_to_idx(current_backend_arg),
        )
        .await?;
    let backend_arg = idx_to_chatops_arg(idx);

    let mut new_config = existing.clone();
    if backend_arg == ChatOpsBackendArg::None {
        new_config.chatops = None;
        return Ok(new_config);
    }
    let default_channel = existing
        .chatops
        .as_ref()
        .map(|c| c.default_channel_id.clone())
        .unwrap_or_default();
    let channel = ask_default(io, "ChatOps default channel id", &default_channel).await?;

    // Build the new ChatOpsConfig. If the operator kept the same provider,
    // preserve all unchanged fields (provider-specific tokens, notification
    // settings, etc.); otherwise start from the wizard's slack default
    // (only slack is implemented end-to-end in the wizard today).
    let preserved = existing
        .chatops
        .as_ref()
        .filter(|c| {
            ChatOpsBackendArg::from_provider(c.provider) == backend_arg
        })
        .cloned();
    let mut chatops = match preserved {
        Some(mut existing_chatops) => {
            existing_chatops.default_channel_id = channel.clone();
            existing_chatops
        }
        None => {
            if backend_arg != ChatOpsBackendArg::Slack {
                bail!(
                    "chatops backend `{}` is experimental and not supported by the wizard yet; pick `none` or `slack`",
                    chatops_backend_label(backend_arg)
                );
            }
            ChatOpsConfig {
                provider: ChatOpsProvider::Slack,
                default_channel_id: channel.clone(),
                notifications: None,
                slack: Some(SlackProviderConfig {
                    bot_token_env: Some("SLACK_BOT_TOKEN".to_string()),
                    bot_token: None,
                    app_token_env: None,
                    app_token: None,
                    listen_channels: Vec::new(),
                    dedup_cache_capacity: crate::config::default_dedup_cache_capacity(),
                    dedup_cache_ttl_secs: crate::config::default_dedup_cache_ttl_secs(),
                }),
                discord: None,
                teams: None,
                mattermost: None,
                matrix: None,
            }
        }
    };
    chatops.default_channel_id = channel;
    new_config.chatops = Some(chatops);
    Ok(new_config)
}

impl ChatOpsBackendArg {
    fn from_provider(p: ChatOpsProvider) -> Self {
        match p {
            ChatOpsProvider::Slack => Self::Slack,
            ChatOpsProvider::Discord => Self::Discord,
            ChatOpsProvider::Teams => Self::Teams,
            ChatOpsProvider::Mattermost => Self::Mattermost,
            ChatOpsProvider::Matrix => Self::Matrix,
        }
    }
}

/// Serialize `new_config` and atomically replace `config_path`. On unix,
/// the new file inherits the prior file's mode and owner where stat
/// allows. `serde_yml` does not preserve comments; the wizard-generated
/// audits block carries none so this is acceptable for `--reconfigure
/// audits` and the operator confirms the diff explicitly for reviewer /
/// chatops.
pub(crate) fn apply_in_place_patch(config_path: &Path, new_config: &Config) -> Result<()> {
    let yaml = serialize_config(new_config)?;
    let parent = config_path
        .parent()
        .ok_or_else(|| anyhow!("config path {} has no parent", config_path.display()))?;
    let tmp = parent.join(format!(
        ".{}.reconfigure.tmp",
        config_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("config.yaml")
    ));

    let prior_mode = prior_file_mode(config_path);

    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)
            .with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(yaml.as_bytes())
            .with_context(|| format!("write {}", tmp.display()))?;
        f.sync_all()
            .with_context(|| format!("fsync {}", tmp.display()))?;
    }

    std::fs::rename(&tmp, config_path).with_context(|| {
        format!("rename {} -> {}", tmp.display(), config_path.display())
    })?;

    #[cfg(unix)]
    if let Some(mode) = prior_mode {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(config_path, std::fs::Permissions::from_mode(mode))
            .with_context(|| format!("restore mode on {}", config_path.display()))?;
    }
    let _ = prior_mode;
    Ok(())
}

#[cfg(unix)]
fn prior_file_mode(p: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).ok().map(|m| m.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn prior_file_mode(_p: &Path) -> Option<u32> {
    None
}

/// Compute a unified diff between the on-disk YAML and the serialized
/// `new_config`, print it via `io`, and prompt `Apply this patch? [y/N]`.
/// On accept, writes the patch via [`apply_in_place_patch`] and returns
/// `Ok(true)`. On decline, the file is unchanged and returns `Ok(false)`.
pub(crate) async fn confirm_diff_and_apply(
    config_path: &Path,
    new_config: &Config,
    io: &mut dyn WizardIo,
) -> Result<bool> {
    let current = std::fs::read_to_string(config_path)
        .with_context(|| format!("read {}", config_path.display()))?;
    let new_yaml = serialize_config(new_config)?;
    let diff = similar::TextDiff::from_lines(&current, &new_yaml);
    let unified = diff
        .unified_diff()
        .header("current", "proposed")
        .to_string();
    io.print("\n");
    if unified.trim().is_empty() {
        io.print("No changes between current and proposed config.\n");
    } else {
        io.print(&unified);
        if !unified.ends_with('\n') {
            io.print("\n");
        }
    }
    let accept = io.confirm("Apply this patch?", false).await?;
    if !accept {
        return Ok(false);
    }
    apply_in_place_patch(config_path, new_config)?;
    Ok(true)
}
