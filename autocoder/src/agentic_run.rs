//! Shared agentic-run primitive (a56).
//!
//! "Wrap a CLI as a subprocess, hand it a prompt, run an agentic session
//! to completion" was implemented five+ times (the executor's
//! `run_subprocess` plus four near-identical audit copies). This module
//! is the single source of truth for that pattern: [`agentic_run`] spawns
//! the child in its own process group, pipes the prompt on stdin, enforces
//! a timeout via the select-and-kill pattern, and returns a unified
//! [`AgenticRunOutcome`]. Streaming-JSON event parsing (`final_answer`,
//! `session_id`, incremental structured log) runs ONLY in
//! [`OutputMode::Streaming`]; [`OutputMode::Capture`] reads stdout/stderr
//! at exit.
//!
//! CLI selection is abstracted behind the [`CliStrategy`] trait so a
//! model's provider can pick the `claude` CLI or the provider-agnostic
//! `opencode` CLI without role code changing. Two strategies are
//! registered: [`ClaudeStrategy`] (Anthropic-shaped, streaming-capable)
//! and [`OpencodeStrategy`] (a60 — any OpenAI-compatible / Ollama
//! endpoint, capture-mode only). A provider that resolves to any other
//! CLI returns a clear "strategy not yet implemented" error
//! ([`strategy_for_provider`]).
//!
//! The refactor is behavior-neutral: the executor keeps streaming-JSON +
//! MCP + the recovery/session-reuse path; each audit keeps simple-capture
//! + no-MCP + its read-only tool list + its ETXTBSY retry.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::executor::event_log::{self, ActionKind, StructuredLogWriter};
use crate::executor::json_event::{self, AssistantBlock, JsonEvent, UserBlock};

/// Unified outcome returned by [`agentic_run`]. Replaces the per-module
/// `SubprocessOutcome` structs the executor and the audits each declared.
///
/// `final_answer` / `session_id` are populated only by the streaming-JSON
/// path (the executor); `streamed_log` is `true` when that path wrote the
/// structured log incrementally (so the legacy `persist_run_log` writer
/// should skip it).
#[derive(Default)]
pub struct AgenticRunOutcome {
    pub timed_out: bool,
    pub exit_status: Option<std::process::ExitStatus>,
    pub stdout: String,
    pub stderr: String,
    /// Agent's conversational summary from the `result` event. `None` in
    /// capture mode AND when a streaming run timed out before the result
    /// event arrived.
    pub final_answer: Option<String>,
    /// Session id captured from the `system`-event init subtype. `None`
    /// in capture mode OR when the system event was absent.
    pub session_id: Option<String>,
    /// `true` when the streaming path built the structured log itself.
    pub streamed_log: bool,
}

/// Output handling for a run. `Streaming` adds `--verbose --output-format
/// stream-json`, parses each event, and writes the structured log
/// incrementally. `Capture` reads stdout/stderr at exit with no parsing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OutputMode {
    Streaming,
    Capture,
}

/// A resolved `(provider, model, api_base_url, api_key)` tuple a strategy
/// translates into that CLI's model-selection mechanism. Constructed from
/// the model registry's resolution of a role's model (a55); `None` at a
/// call site preserves the CLI's own default-model behavior.
pub struct ResolvedModel {
    /// The model's provider. The `claude` strategy carries it only for
    /// dispatch (its `apply_model_selection` keys off env, not provider);
    /// the `opencode` strategy reads it to build `--model <provider>/<model>`
    /// AND the `opencode.json` provider id.
    pub provider: crate::config::LlmProvider,
    pub model: String,
    pub api_base_url: String,
    /// The resolved LLM credential. a003: NO `CliStrategy` propagates this to
    /// the wrapped subprocess — neither into a workspace config file
    /// (`opencode.json`) nor into the subprocess env (`ANTHROPIC_AUTH_TOKEN`).
    /// The CLI authenticates itself; the model is tunneled across that
    /// connection. The key is consumed only by autocoder's in-process HTTP
    /// clients (the `oneshot` reviewer's `LlmClient`), which resolve their key
    /// directly and never spawn a subprocess. A strategy that receives a model
    /// carrying a non-empty key ignores the key (see
    /// [`cli_role_unused_key_warning`] for the startup notice).
    pub api_key: String,
}

/// Sandbox configuration for a run: the allowed-tools list, the disallowed
/// bash/read patterns, AND whether `Write`/`Edit` are denied in the
/// settings file. The executor allows writes (`deny_writes: false`); the
/// read-only audits deny them (`deny_writes: true`).
pub struct SandboxConfig {
    pub allowed_tools: Vec<String>,
    pub disallowed_bash_patterns: Vec<String>,
    pub disallowed_read_paths: Vec<String>,
    pub deny_writes: bool,
}

/// Context a [`CliStrategy`] reads when building the invocation. The
/// claude-format settings file has already been written by [`agentic_run`];
/// the `claude` strategy references it and only assembles argv. The
/// `opencode` strategy ignores `settings_path` (opencode uses its own
/// `opencode.json` permission config, which it writes from this context).
pub struct BuildContext<'a> {
    pub settings_path: &'a Path,
    pub allowed_tools: &'a [String],
    /// Append the autocoder MCP provided-tool names to `--allowedTools`
    /// (the executor's main path does this so the agent may call the
    /// `ask_user` / `outcome_*` / `query_canonical_specs` MCP tools).
    pub include_autocoder_tools: bool,
    /// Emit `--verbose --output-format stream-json` on the command.
    pub emit_stream_json: bool,
    /// `--resume <session_id>` for the recovery turn (claude only).
    pub resume_session_id: Option<&'a str>,
    /// The run's workspace. The `opencode` strategy writes `opencode.json`
    /// here (MCP block + provider config + permissions); the `claude`
    /// strategy does not read it (its caller writes `.mcp.json`).
    pub workspace: &'a Path,
    /// The MCP role this run serves (a56): the value written as
    /// `ORCH_MCP_ROLE` (and the submission-store key) into the `opencode`
    /// strategy's `opencode.json` `mcp` block so the role's `submit_*` tool
    /// is reachable. `None` → no submission tool is advertised. The
    /// `claude` strategy ignores it (its caller writes the MCP env via
    /// `write_mcp_config`).
    pub mcp_role: Option<&'a str>,
    /// The resolved model, so the `opencode` strategy can write the
    /// provider config (model + base URL, NEVER the `api_key` — a003) into
    /// `opencode.json`. `None` preserves the CLI's own default-model
    /// behavior. The `claude` strategy ignores it here (it sets the
    /// non-credential `ANTHROPIC_BASE_URL` / `ANTHROPIC_MODEL` env in
    /// `apply_model_selection` instead).
    pub model: Option<&'a ResolvedModel>,
}

/// Abstracts CLI invocation so a model's provider can determine the CLI
/// without role code changing. Two jobs: build the invocation (binary,
/// flags, allowed-tools/settings format) AND translate a [`ResolvedModel`]
/// into the CLI's model-selection mechanism.
pub trait CliStrategy: Send + Sync {
    fn build_command(&self, ctx: &BuildContext<'_>) -> Command;
    fn apply_model_selection(&self, cmd: &mut Command, model: Option<&ResolvedModel>);
}

/// Build the `--allowedTools` value Claude CLI expects. When
/// `include_autocoder_tools` is set, the autocoder MCP provided-tool names
/// (`mcp__ask_user__*`) are appended so the daemon's contract tools are
/// always allowed regardless of the operator's `allowed_tools` list.
pub(crate) fn build_allowed_tools_value(
    allowed: &[String],
    include_autocoder_tools: bool,
) -> String {
    let mut combined: Vec<String> = allowed.to_vec();
    if include_autocoder_tools {
        for tool in crate::mcp_askuser_server::PROVIDED_TOOL_NAMES {
            combined.push(crate::mcp_askuser_server::qualified_tool_name(tool));
        }
    }
    combined.join(",")
}

/// The `claude` CLI strategy. Reproduces the pre-refactor invocation
/// exactly: `--settings <file>`, `--allowedTools <combined>`,
/// `--permission-mode acceptEdits`, optional `--resume`, and — in
/// streaming mode — `--verbose --output-format stream-json`. Model
/// selection sets `ANTHROPIC_BASE_URL` / `ANTHROPIC_MODEL` ONLY when a
/// model is configured; with no model it sets neither (the executor's
/// current CLI-default behavior).
///
/// a003: model selection sets NO `ANTHROPIC_AUTH_TOKEN`. The resolved
/// `api_key` is a credential and never reaches the subprocess — claude
/// authenticates from its own login / credential store (`claude login`),
/// and the model is tunneled across that connection. An env-set auth token
/// would be readable from the agent's Bash AND (for Anthropic) would force
/// pay-per-token off the operator's subscription. `ANTHROPIC_BASE_URL` /
/// `ANTHROPIC_MODEL` are endpoint/model selection, NOT credentials, so they
/// remain.
pub struct ClaudeStrategy {
    pub command: String,
    pub args: Vec<String>,
}

impl ClaudeStrategy {
    pub fn new(command: String, args: Vec<String>) -> Self {
        Self { command, args }
    }
}

impl CliStrategy for ClaudeStrategy {
    fn build_command(&self, ctx: &BuildContext<'_>) -> Command {
        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args)
            .arg("--settings")
            .arg(ctx.settings_path)
            .arg("--allowedTools")
            .arg(build_allowed_tools_value(
                ctx.allowed_tools,
                ctx.include_autocoder_tools,
            ))
            .arg("--permission-mode")
            .arg("acceptEdits");
        if let Some(sid) = ctx.resume_session_id {
            cmd.arg("--resume").arg(sid);
        }
        if ctx.emit_stream_json {
            // `--verbose` is required by Claude CLI alongside `stream-json`
            // for non-interactive sessions; without it the CLI emits a
            // single result envelope rather than streaming events.
            cmd.arg("--verbose")
                .arg("--output-format")
                .arg("stream-json");
        }
        cmd
    }

    fn apply_model_selection(&self, cmd: &mut Command, model: Option<&ResolvedModel>) {
        if let Some(m) = model {
            // Endpoint + model selection only. a003: NO `ANTHROPIC_AUTH_TOKEN`
            // — the resolved `m.api_key` is a credential the model never needs,
            // so it is never placed in the subprocess env. claude authenticates
            // from its own login / credential store.
            cmd.env("ANTHROPIC_BASE_URL", &m.api_base_url);
            cmd.env("ANTHROPIC_MODEL", &m.model);
        }
        // model: None → set nothing; the CLI uses its own default model.
    }
}

/// Filename of the opencode config the [`OpencodeStrategy`] writes into the
/// workspace. opencode auto-discovers `opencode.json` from the project root
/// (the run's working directory, set by [`agentic_run`]).
const OPENCODE_CONFIG_FILENAME: &str = "opencode.json";

/// The `opencode` CLI strategy (a60). Builds `opencode run` invocations for
/// the provider-agnostic `opencode` CLI so a role whose model resolves to
/// `opencode` (a55's `provider → CLI` rule for `openai_compatible`/`ollama`,
/// OR an explicit `cli: opencode`) runs agentically instead of erroring.
///
/// Unlike [`ClaudeStrategy`], opencode carries everything in one workspace
/// config file, `opencode.json`: the MCP `mcp` block (`type: local`, the
/// MCP-child command, env including `ORCH_MCP_ROLE`), the resolved provider
/// config (model + base URL, NEVER the `api_key` — a003: a credential in a
/// workspace file could be committed, AND the model never needs it), AND a
/// `permission` block mapped from a56's sandbox.
/// [`OpencodeStrategy::build_command`] writes that file; model
/// selection is `--model <provider>/<model>` (NOT `ANTHROPIC_*` env). It
/// writes NO `.mcp.json` (the `claude` MCP format). The prompt is delivered
/// on stdin — [`agentic_run`] already pipes it, AND headless `opencode run`
/// reads its message from piped stdin — so `build_command` appends no
/// positional message (which would also risk `ARG_MAX` on large review
/// prompts; see the integration spike notes).
///
/// opencode is capture-mode only; the streaming-JSON event path
/// (`final_answer` / `session_id` / incremental log) stays claude-specific.
pub struct OpencodeStrategy {
    pub command: String,
    pub args: Vec<String>,
}

impl OpencodeStrategy {
    pub fn new(command: String, args: Vec<String>) -> Self {
        Self { command, args }
    }

    /// The MCP child's env map for `opencode.json` (`mcp.<server>.environment`).
    /// Mirrors `ClaudeCliExecutor::write_mcp_config`: always the workspace;
    /// the role (as both `ORCH_MCP_CHANGE` submission key AND `ORCH_MCP_ROLE`)
    /// when a role is set; the daemon control-socket vars when the parent
    /// process carries them (canonical_rag configured).
    fn mcp_environment(ctx: &BuildContext<'_>) -> serde_json::Value {
        let mut env = serde_json::Map::new();
        env.insert(
            crate::mcp_askuser_server::ENV_WORKSPACE.to_string(),
            serde_json::Value::String(ctx.workspace.to_string_lossy().into_owned()),
        );
        if let Some(role) = ctx.mcp_role {
            // For the submission roles the change name AND the role name are
            // the same value (the reviewer/contradiction call sites pass
            // their role as both); see `write_mcp_config`.
            env.insert(
                crate::mcp_askuser_server::ENV_CHANGE.to_string(),
                serde_json::Value::String(role.to_string()),
            );
            env.insert(
                crate::mcp_askuser_server::ENV_ROLE.to_string(),
                serde_json::Value::String(role.to_string()),
            );
        }
        if let Ok(socket) = std::env::var(crate::mcp_askuser_server::ENV_CONTROL_SOCKET) {
            env.insert(
                crate::mcp_askuser_server::ENV_CONTROL_SOCKET.to_string(),
                serde_json::Value::String(socket),
            );
            let basename = std::env::var(crate::mcp_askuser_server::ENV_WORKSPACE_BASENAME)
                .unwrap_or_else(|_| {
                    ctx.workspace
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown_workspace")
                        .to_string()
                });
            env.insert(
                crate::mcp_askuser_server::ENV_WORKSPACE_BASENAME.to_string(),
                serde_json::Value::String(basename),
            );
        }
        serde_json::Value::Object(env)
    }

    /// Map a56's allowed-tools list onto opencode's `permission` block. Each
    /// permission opencode gates is `"allow"` when the equivalent tool is in
    /// the allowed list, else `"deny"`. A read-only sandbox
    /// (`["Read","Glob","Grep"]`) therefore denies `edit` (file mutation —
    /// opencode's `edit` permission governs both its `write` and `edit`
    /// tools), `bash`, AND `webfetch`. The always-available read tools
    /// (read/grep/glob) are not permission-gated; the role's `submit_*` tool
    /// is exposed via the `mcp` block.
    fn permission_block(allowed_tools: &[String]) -> serde_json::Value {
        let allows = |name: &str| allowed_tools.iter().any(|t| t.eq_ignore_ascii_case(name));
        let verdict = |allowed: bool| if allowed { "allow" } else { "deny" };
        serde_json::json!({
            "edit": verdict(allows("Edit") || allows("Write")),
            "bash": verdict(allows("Bash")),
            "webfetch": verdict(allows("WebFetch")),
        })
    }

    /// The `provider` block for the resolved model, keyed by the provider's
    /// id (`openai_compatible` / `ollama`) so it matches the `--model
    /// <provider>/<model>` selection. `None` when no model is configured
    /// (opencode uses its own default).
    ///
    /// a003: the resolved `api_key` is NEVER written here. `opencode.json`
    /// lives at the workspace root and is not git-excluded, so a key in it
    /// could be committed; more fundamentally the model never needs the
    /// credential. opencode authenticates from its own out-of-band provider
    /// config / login (e.g. opencode → OpenRouter), so only the provider's
    /// model + base URL are written. (Ollama never authenticated anyway.)
    fn provider_block(model: Option<&ResolvedModel>) -> Option<serde_json::Value> {
        let m = model?;
        let provider_id = m.provider.as_str();
        let mut options = serde_json::Map::new();
        options.insert(
            "baseURL".to_string(),
            serde_json::Value::String(m.api_base_url.clone()),
        );
        // No `apiKey` — see the doc comment above (a003).
        let mut models = serde_json::Map::new();
        models.insert(m.model.clone(), serde_json::json!({}));
        let mut entry = serde_json::Map::new();
        entry.insert(
            "npm".to_string(),
            serde_json::Value::String("@ai-sdk/openai-compatible".to_string()),
        );
        entry.insert(
            "name".to_string(),
            serde_json::Value::String(provider_id.to_string()),
        );
        entry.insert("options".to_string(), serde_json::Value::Object(options));
        entry.insert("models".to_string(), serde_json::Value::Object(models));
        let mut provider = serde_json::Map::new();
        provider.insert(provider_id.to_string(), serde_json::Value::Object(entry));
        Some(serde_json::Value::Object(provider))
    }

    /// Assemble the full `opencode.json` value: the `mcp` block, the
    /// `permission` block, AND (when a model is resolved) the `provider`
    /// block.
    fn config_value(ctx: &BuildContext<'_>) -> Result<serde_json::Value> {
        // We may be running from a non-autocoder binary (e.g. cargo test).
        // `current_exe` is the actual running binary; in production the
        // `autocoder` binary, whose `mcp-ask-user-server` subcommand the MCP
        // child runs.
        let exe = std::env::current_exe()
            .context("resolving current autocoder binary path for opencode MCP config")?;
        let mut server = serde_json::Map::new();
        server.insert(
            "type".to_string(),
            serde_json::Value::String("local".to_string()),
        );
        server.insert(
            "command".to_string(),
            serde_json::json!([exe.to_string_lossy(), "mcp-ask-user-server"]),
        );
        server.insert("environment".to_string(), Self::mcp_environment(ctx));
        server.insert("enabled".to_string(), serde_json::Value::Bool(true));

        let mut mcp = serde_json::Map::new();
        mcp.insert(
            crate::mcp_askuser_server::SERVER_NAME.to_string(),
            serde_json::Value::Object(server),
        );

        let mut config = serde_json::Map::new();
        config.insert(
            "$schema".to_string(),
            serde_json::Value::String("https://opencode.ai/config.json".to_string()),
        );
        config.insert("mcp".to_string(), serde_json::Value::Object(mcp));
        config.insert(
            "permission".to_string(),
            Self::permission_block(ctx.allowed_tools),
        );
        if let Some(provider) = Self::provider_block(ctx.model) {
            config.insert("provider".to_string(), provider);
        }
        Ok(serde_json::Value::Object(config))
    }

    /// Write `<workspace>/opencode.json`. `pub(crate)` so callers that wire
    /// opencode end-to-end can reuse the exact shape; returns the path.
    pub(crate) fn write_config(ctx: &BuildContext<'_>) -> Result<PathBuf> {
        let value = Self::config_value(ctx)?;
        let path = ctx.workspace.join(OPENCODE_CONFIG_FILENAME);
        let raw = serde_json::to_string_pretty(&value)?;
        std::fs::write(&path, raw)
            .with_context(|| format!("writing opencode config {}", path.display()))?;
        Ok(path)
    }
}

impl CliStrategy for OpencodeStrategy {
    fn build_command(&self, ctx: &BuildContext<'_>) -> Command {
        // Write the workspace `opencode.json` (MCP + permissions + provider).
        // Best-effort: a write failure is logged but does not abort argv
        // assembly (the run will surface the missing-config error itself).
        if let Err(e) = Self::write_config(ctx) {
            tracing::warn!(
                workspace = %ctx.workspace.display(),
                "failed to write opencode.json (run continues): {e:#}"
            );
        }
        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args).arg("run");
        // The prompt is delivered on stdin by `agentic_run`; `opencode run`
        // reads its message from piped stdin, so no positional message is
        // appended here. `resume_session_id` is the claude recovery
        // mechanism (streaming) and is not used by the capture-mode opencode
        // roles, so it is intentionally ignored.
        cmd
    }

    fn apply_model_selection(&self, cmd: &mut Command, model: Option<&ResolvedModel>) {
        if let Some(m) = model {
            cmd.arg("--model")
                .arg(format!("{}/{}", m.provider.as_str(), m.model));
        }
        // No `ANTHROPIC_*` env — that is the claude strategy's mechanism;
        // opencode reads the provider config from `opencode.json` (written in
        // `build_command`) AND the `--model <provider>/<model>` selection.
    }
}

/// Resolve a role's strategy from the model's provider via a55's
/// `provider → default CLI` rule ([`crate::config::default_cli_for`]).
///
/// Forward-looking API: the agentic roles that resolve a model per-role
/// (changes 4–8) call this; this change registers the rule + the `claude`
/// strategy AND exercises both via tests.
#[allow(dead_code)]
pub fn strategy_for_provider(
    provider: crate::config::LlmProvider,
    command: String,
    args: Vec<String>,
) -> Result<Box<dyn CliStrategy>> {
    strategy_for_cli(crate::config::default_cli_for(provider), command, args)
}

/// Resolve the strategy for a specific CLI. `claude` (a56) AND `opencode`
/// (a60) are registered; both map to a real strategy with no subprocess
/// spawned at resolution time. The `Result` is retained so a future CLI can
/// land an error arm without changing call sites.
#[allow(dead_code)]
pub fn strategy_for_cli(
    cli: crate::config::CliKind,
    command: String,
    args: Vec<String>,
) -> Result<Box<dyn CliStrategy>> {
    match cli {
        crate::config::CliKind::Claude => Ok(Box::new(ClaudeStrategy::new(command, args))),
        crate::config::CliKind::Opencode => Ok(Box::new(OpencodeStrategy::new(command, args))),
    }
}

/// Startup WARN for a role that resolves to a [`CliStrategy`] but carries a
/// configured `api_key` (a003). A CLI role authenticates from the wrapped
/// CLI's own login / credential store, so the resolved key is NEVER passed to
/// the subprocess — the strategy ignores it. That makes the configured key
/// dead config; this returns the one-line WARN the daemon logs exactly once at
/// startup so the operator can remove it. Returns `None` when no key is
/// configured (`has_key == false`).
///
/// Roles that use autocoder's in-process HTTP path (e.g. the `oneshot`
/// reviewer's `LlmClient`) resolve and use their key directly and must NOT
/// call this — their key is genuinely consumed, not dead. Separated from the
/// logging site (`cli::run` startup) as a pure decision so tests assert the
/// disposition without a daemon, mirroring
/// [`crate::code_reviewer::startup_reviewer_kind_decision`].
pub fn cli_role_unused_key_warning(role_label: &str, has_key: bool) -> Option<String> {
    has_key.then(|| {
        format!(
            "role `{role_label}` has a configured `api_key`, but it resolves to a CLI \
             strategy whose wrapped CLI authenticates from its own login / credential \
             store — the key is UNUSED for CLI roles and is never passed to the \
             subprocess (neither a workspace config file nor the env). Remove the \
             `api_key` from this role's config to silence this warning."
        )
    })
}

/// Everything [`agentic_run`] needs for one run. Most call sites set only
/// a handful of fields; the rest carry safe per-caller defaults that
/// preserve each pre-refactor path's exact behavior.
pub struct AgenticRunOpts<'a> {
    pub workspace: &'a Path,
    /// Log identifier (the change name, or a synthetic name for non-change
    /// flows). Used only by streaming mode to compute the structured-log
    /// path.
    pub change: &'a str,
    pub strategy: &'a dyn CliStrategy,
    pub prompt: &'a str,
    pub sandbox: SandboxConfig,
    pub model: Option<&'a ResolvedModel>,
    pub output_mode: OutputMode,
    pub timeout: Duration,
    /// Daemon paths (for the structured-log path AND the busy-marker
    /// sidecar). `None` for the audits, which capture-only and write no
    /// sidecar.
    pub paths: Option<&'a Arc<crate::paths::DaemonPaths>>,
    pub settings_dir: Option<&'a Path>,
    /// Append the autocoder MCP provided-tool names to `--allowedTools`.
    pub include_autocoder_tools: bool,
    /// Emit `--verbose --output-format stream-json` even in capture mode
    /// (the recovery turn emits stream-json but reads it at exit). Ignored
    /// in streaming mode, which always emits the flags.
    pub emit_stream_json_in_capture: bool,
    /// `--resume <session_id>` for the recovery turn's session reuse.
    pub resume_session_id: Option<&'a str>,
    /// Write the busy-marker subprocess-PID sidecar (the executor paths,
    /// so stuck-state recovery can `killpg` the child's group). Audits
    /// do not.
    pub track_subprocess_marker: bool,
    /// Spawn via the ETXTBSY-retry helper (the audits, which race parallel
    /// test fixtures writing sibling scripts). The executor uses a plain
    /// spawn.
    pub etxtbsy_retry_spawn: bool,
    /// a006: the OS-level sandbox to wrap this spawn in. `enforce == false`
    /// (the default) skips the OS layer entirely (test fixtures); production
    /// call sites set an enforced [`crate::sandbox::RunSandbox`] so EVERY role
    /// is wrapped and no role can opt out. When enforced with no available
    /// mechanism AND no operator opt-in, the spawn fails closed.
    pub os_sandbox: crate::sandbox::RunSandbox,
}

/// Spawn the wrapped CLI, write `prompt` on its stdin, wait with the
/// configured timeout, AND return the unified outcome. See the module
/// docs for the behavior contract.
pub async fn agentic_run(opts: AgenticRunOpts<'_>) -> Result<AgenticRunOutcome> {
    // a006 fail-closed gate (task 4.1): when the OS sandbox is enforced but no
    // mechanism is available AND the operator has not opted into unsandboxed
    // operation, refuse to spawn — BEFORE writing any settings or building the
    // command. `None` here means the OS layer is not enforced for this run.
    let spawn_plan = if opts.os_sandbox.enforce {
        Some(
            crate::sandbox::decide_spawn(
                opts.os_sandbox.mechanism,
                opts.os_sandbox.allow_unsandboxed,
            )
            .context("OS-level sandbox mechanism gate")?,
        )
    } else {
        None
    };

    // a006 engine_deny (task 5.2): extend the per-invocation read-deny set to
    // every registered CLI store (self included) so the agent's `Read`/`Bash`
    // tools are denied those paths at the CLI permission layer. Supplied
    // per-invocation through the settings file below — never by mutating the
    // operator's global CLI config.
    let mut disallowed_read_paths = opts.sandbox.disallowed_read_paths.clone();
    if opts.os_sandbox.enforce {
        disallowed_read_paths.extend(opts.os_sandbox.engine_deny_paths());
    }
    let resolved_sandbox = crate::config::ResolvedSandbox {
        allowed_tools: opts.sandbox.allowed_tools.clone(),
        disallowed_bash_patterns: opts.sandbox.disallowed_bash_patterns.clone(),
        disallowed_read_paths,
    };
    let (settings_path, _settings_guard) = crate::audits::write_sandbox_settings(
        &resolved_sandbox,
        opts.settings_dir,
        opts.sandbox.deny_writes,
    )
    .context("generating sandbox settings file")?;

    let streaming = matches!(opts.output_mode, OutputMode::Streaming);
    let emit_stream_json = streaming || opts.emit_stream_json_in_capture;

    // The command is pure argv assembly (no IO), so it can be rebuilt on
    // each ETXTBSY retry attempt. The settings file is written exactly
    // once, above.
    let build = || {
        let ctx = BuildContext {
            settings_path: &settings_path,
            allowed_tools: &opts.sandbox.allowed_tools,
            include_autocoder_tools: opts.include_autocoder_tools,
            emit_stream_json,
            resume_session_id: opts.resume_session_id,
            workspace: opts.workspace,
            model: opts.model,
            // The submission roles that drive opencode (reviewer a58,
            // contradiction check a59) currently write their own `.mcp.json`
            // via `write_mcp_config` and key the role there; threading the
            // role through to the opencode strategy's `opencode.json` writer
            // is the call-site change those roles make when they opt into
            // opencode end-to-end. This change registers the strategy AND
            // exposes the seam (`BuildContext::mcp_role`); it does not modify
            // a58/a59, so the production build leaves it `None`.
            mcp_role: None,
        };
        let mut inner_cmd = opts.strategy.build_command(&ctx);
        opts.strategy.apply_model_selection(&mut inner_cmd, opts.model);

        // a006 (tasks 2.1–2.5, 3.1): wrap the strategy command in the OS-level
        // sandbox via the resolved mechanism. The wrapper preserves stdio +
        // process-group + timeout/kill behavior unchanged — the `--pipe` /
        // bwrap pass-through keeps streaming-JSON and capture modes intact.
        // `Unsandboxed` (operator opt-in) and the not-enforced path spawn the
        // strategy command directly.
        let mut cmd = match spawn_plan {
            Some(crate::sandbox::SpawnPlan::Wrap(mechanism)) => {
                let inner = crate::sandbox::InnerCommand::from_command(&inner_cmd);
                // a013: the program (resolved + bound under an allowlist policy
                // so the wrapped CLI execs under a masked home) drives the plan.
                let plan = opts.os_sandbox.build_plan(opts.workspace, &inner.program);
                crate::sandbox::wrap_command(mechanism, &plan, &inner)
            }
            _ => inner_cmd,
        };
        cmd.current_dir(opts.workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Own process group so stuck-state recovery can `killpg` the
            // whole subprocess tree with one signal. `process_group(0)` is
            // stable Rust.
            .process_group(0);
        cmd
    };

    let mut child = if opts.etxtbsy_retry_spawn {
        crate::audits::spawn_with_etxtbsy_retry(build)
            .await
            .context("spawning agentic-run subprocess")?
    } else {
        build().spawn().context("spawning agentic-run subprocess")?
    };

    // Record the spawned child's PID to a sidecar so the busy-marker
    // stuck-state recovery has a kill target covering the child's process
    // group. The guard cleans the file on every exit path.
    let _subprocess_marker_guard = if opts.track_subprocess_marker {
        match (opts.paths, child.id()) {
            (Some(paths), Some(pid)) => {
                if let Err(e) =
                    crate::busy_marker::write_subprocess_marker(paths, opts.workspace, pid)
                {
                    tracing::warn!(
                        workspace = %opts.workspace.display(),
                        pid,
                        "failed to write subprocess sidecar marker (run continues): {e:#}"
                    );
                    None
                } else {
                    Some(SubprocessMarkerGuard {
                        paths: paths.clone(),
                        workspace: opts.workspace.to_path_buf(),
                    })
                }
            }
            _ => None,
        }
    } else {
        None
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(opts.prompt.as_bytes()).await;
    }
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    if streaming {
        // Streaming requires the daemon paths for the structured-log path.
        // Production always provides them; fall back to capture if absent.
        if let Some(paths) = opts.paths {
            run_streaming(
                child,
                stdout_pipe,
                stderr_pipe,
                paths,
                opts.workspace,
                opts.change,
                opts.prompt,
                opts.timeout,
            )
            .await
        } else {
            run_capture(child, stdout_pipe, stderr_pipe, opts.timeout).await
        }
    } else {
        run_capture(child, stdout_pipe, stderr_pipe, opts.timeout).await
    }
}

/// Capture path: wait for child exit (or timeout) then read stdout +
/// stderr in one shot. No structured log is written.
async fn run_capture(
    mut child: tokio::process::Child,
    mut stdout_pipe: Option<tokio::process::ChildStdout>,
    mut stderr_pipe: Option<tokio::process::ChildStderr>,
    timeout: Duration,
) -> Result<AgenticRunOutcome> {
    let sleeper = tokio::time::sleep(timeout);
    tokio::pin!(sleeper);

    let exit_status: Option<std::io::Result<std::process::ExitStatus>> = tokio::select! {
        biased;
        () = &mut sleeper => None,
        res = child.wait() => Some(res),
    };

    match exit_status {
        None => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            Ok(AgenticRunOutcome {
                timed_out: true,
                exit_status: None,
                stdout: String::new(),
                stderr: "timeout".to_string(),
                ..Default::default()
            })
        }
        Some(Err(e)) => Err(e).context("waiting on agentic-run child process"),
        Some(Ok(status)) => {
            let mut stdout_text = String::new();
            if let Some(ref mut p) = stdout_pipe {
                let _ = p.read_to_string(&mut stdout_text).await;
            }
            let mut stderr_text = String::new();
            if let Some(ref mut p) = stderr_pipe {
                let _ = p.read_to_string(&mut stderr_text).await;
            }
            Ok(AgenticRunOutcome {
                timed_out: false,
                exit_status: Some(status),
                stdout: stdout_text,
                stderr: stderr_text,
                ..Default::default()
            })
        }
    }
}

/// Streaming path: open the structured log writer, spawn one task that
/// reads stdout line-by-line and dispatches parsed events to the log + one
/// task that reads stderr into the writer's buffer, then race
/// `child.wait()` against the timeout. On timeout-kill the partial action
/// stream is already on disk; the writer is `finalize`d unconditionally.
#[allow(clippy::too_many_arguments)]
async fn run_streaming(
    mut child: tokio::process::Child,
    stdout_pipe: Option<tokio::process::ChildStdout>,
    stderr_pipe: Option<tokio::process::ChildStderr>,
    paths: &crate::paths::DaemonPaths,
    workspace: &Path,
    change: &str,
    prompt: &str,
    timeout: Duration,
) -> Result<AgenticRunOutcome> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let log_path = crate::executor::claude_cli::run_log_path(paths, workspace, change);
    let writer = match event_log::open(&log_path) {
        Ok(w) => Arc::new(w),
        Err(e) => {
            tracing::warn!(
                log_file = %log_path.display(),
                "could not open structured log; falling back to capture: {e:#}"
            );
            return run_capture(child, stdout_pipe, stderr_pipe, timeout).await;
        }
    };
    if let Err(e) = writer.write_prompt(prompt) {
        tracing::warn!(
            log_file = %log_path.display(),
            "writing prompt header to structured log failed: {e:#}"
        );
    }

    // Stdout reader: parse one JSON event per line; dispatch each to the
    // structured log. Accumulate the raw lines too so callers' `stdout`
    // still reflects what was emitted (sentinel extraction, heuristics).
    let stdout_writer = writer.clone();
    let stdout_handle: tokio::task::JoinHandle<String> = match stdout_pipe {
        Some(pipe) => tokio::spawn(async move {
            let mut buf = String::new();
            let mut reader = BufReader::new(pipe).lines();
            loop {
                match reader.next_line().await {
                    Ok(Some(line)) => {
                        buf.push_str(&line);
                        buf.push('\n');
                        dispatch_event_to_log(&stdout_writer, &line);
                    }
                    Ok(None) => break,
                    Err(e) => {
                        tracing::warn!("stdout reader error: {e}");
                        break;
                    }
                }
            }
            buf
        }),
        None => tokio::spawn(async { String::new() }),
    };

    // Stderr reader: stream bytes into the writer's buffer so the STDERR
    // section's annotation reflects the true byte count.
    let stderr_writer = writer.clone();
    let stderr_handle: tokio::task::JoinHandle<String> = match stderr_pipe {
        Some(mut pipe) => tokio::spawn(async move {
            let mut buf = String::new();
            let mut chunk = [0u8; 4096];
            loop {
                match pipe.read(&mut chunk).await {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.push_str(&String::from_utf8_lossy(&chunk[..n]));
                        let _ = stderr_writer.append_stderr(&chunk[..n]);
                    }
                    Err(e) => {
                        tracing::warn!("stderr reader error: {e}");
                        break;
                    }
                }
            }
            buf
        }),
        None => tokio::spawn(async { String::new() }),
    };

    let sleeper = tokio::time::sleep(timeout);
    tokio::pin!(sleeper);

    let exit_status: Option<std::io::Result<std::process::ExitStatus>> = tokio::select! {
        biased;
        () = &mut sleeper => None,
        res = child.wait() => Some(res),
    };

    let timed_out = exit_status.is_none();
    let status_opt: Option<std::process::ExitStatus> = match exit_status {
        None => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            None
        }
        Some(Err(e)) => return Err(e).context("waiting on agentic-run child process"),
        Some(Ok(s)) => Some(s),
    };

    // The reader tasks return when their pipe hits EOF (the child closed
    // its end). After `child.wait()` / `start_kill()` the child is reaped;
    // awaiting the readers is safe.
    let stdout_text = stdout_handle.await.unwrap_or_default();
    let stderr_text = stderr_handle.await.unwrap_or_default();

    // Flush the structured log AFTER readers finished so the FINAL ANSWER
    // section reflects whatever set_final_answer captured.
    if let Err(e) = writer.finalize() {
        tracing::warn!(
            log_file = %log_path.display(),
            "finalizing structured log failed: {e:#}"
        );
    }
    let final_answer = writer.final_answer();
    let session_id = writer.session_id();

    Ok(AgenticRunOutcome {
        timed_out,
        exit_status: status_opt,
        stdout: stdout_text,
        stderr: if timed_out && stderr_text.is_empty() {
            "timeout".to_string()
        } else {
            stderr_text
        },
        final_answer,
        session_id,
        streamed_log: true,
    })
}

/// RAII guard that removes the subprocess-PID sidecar when dropped, so the
/// next iteration's busy-marker recovery only sees a sidecar when an actual
/// orphan exists (the daemon crashed before Drop ran).
struct SubprocessMarkerGuard {
    paths: Arc<crate::paths::DaemonPaths>,
    workspace: std::path::PathBuf,
}

impl Drop for SubprocessMarkerGuard {
    fn drop(&mut self) {
        crate::busy_marker::remove_subprocess_marker(&self.paths, &self.workspace);
    }
}

// ---------------------------------------------------------------------------
// Streaming-JSON event dispatch (moved from `executor::claude_cli`).
// ---------------------------------------------------------------------------

/// Parse a stdout line as a JSON event and append a corresponding
/// ACTIONS-section line (or, for the `result` event, capture the final
/// answer in the writer's buffer). Malformed JSON lands as `[raw]`;
/// unknown event types as `[unknown:<type>]` — neither aborts the
/// stream-reader loop.
fn dispatch_event_to_log(writer: &StructuredLogWriter, line: &str) {
    if line.is_empty() {
        return;
    }
    match json_event::parse_event_line(line) {
        Ok(event) => dispatch_parsed_event(writer, event),
        Err(e) => {
            tracing::warn!("claude stream-json: malformed line, recording as [raw]: {e}");
            let _ = writer.append_action(ActionKind::Raw, line);
        }
    }
}

fn dispatch_parsed_event(writer: &StructuredLogWriter, event: JsonEvent) {
    match event {
        JsonEvent::System { content } => {
            // Init metadata isn't actionable for operators; suppress from
            // the action stream. We DO capture the session_id so the
            // recovery loop can `claude --resume <session_id>`.
            if let Some(id) = content.get("session_id").and_then(|v| v.as_str())
                && !id.is_empty()
            {
                writer.set_session_id(id.to_string());
            }
        }
        JsonEvent::Assistant { content_blocks } => {
            for block in content_blocks {
                match block {
                    AssistantBlock::Text { text } => {
                        for line in wrap_assistant_text(&text) {
                            let _ = writer.append_action(ActionKind::Assistant, &line);
                        }
                    }
                    AssistantBlock::ToolUse {
                        tool_name,
                        tool_input,
                    } => {
                        let summary = format_tool_input_summary(&tool_input);
                        let content = if summary.is_empty() {
                            tool_name
                        } else {
                            format!("{tool_name} {summary}")
                        };
                        let _ = writer.append_action(ActionKind::ToolUse, &content);
                    }
                }
            }
        }
        JsonEvent::User { content_blocks } => {
            for block in content_blocks {
                match block {
                    UserBlock::ToolResult {
                        content, is_error, ..
                    } => {
                        if is_error {
                            let msg: String = content.chars().take(200).collect();
                            let _ = writer.append_action(
                                ActionKind::Unknown("tool_result:error".into()),
                                &msg,
                            );
                        } else {
                            let line = format!("({n} bytes returned)", n = content.len());
                            let _ = writer.append_action(ActionKind::ToolResult, &line);
                        }
                    }
                }
            }
        }
        JsonEvent::Result { final_text, .. } => {
            let _ = writer.set_final_answer(final_text);
        }
        JsonEvent::Unknown { event_type, raw } => {
            let body = serde_json::to_string(&raw).unwrap_or_default();
            let _ = writer.append_action(ActionKind::Unknown(event_type), &body);
        }
    }
}

/// Wrap assistant text at ~80 columns on whitespace boundaries; long
/// single-line runs (URLs, code) get returned as a single chunk to avoid
/// mid-token splits.
fn wrap_assistant_text(text: &str) -> Vec<String> {
    const WIDTH: usize = 80;
    let mut out: Vec<String> = Vec::new();
    for para in text.split('\n') {
        if para.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in para.split_whitespace() {
            if current.is_empty() {
                current.push_str(word);
            } else if current.len() + 1 + word.len() <= WIDTH {
                current.push(' ');
                current.push_str(word);
            } else {
                out.push(std::mem::take(&mut current));
                current.push_str(word);
            }
        }
        if !current.is_empty() {
            out.push(current);
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Format a `tool_input` JSON value into a one-line summary suitable for a
/// `[tool_use]` log line. Truncates at ~200 chars to keep the log readable.
fn format_tool_input_summary(input: &serde_json::Value) -> String {
    let raw = match input {
        serde_json::Value::Object(map) => {
            if let Some(p) = map.get("file_path").and_then(|v| v.as_str()) {
                p.to_string()
            } else if let Some(p) = map.get("path").and_then(|v| v.as_str()) {
                p.to_string()
            } else if let Some(c) = map.get("command").and_then(|v| v.as_str()) {
                c.to_string()
            } else if let Some(p) = map.get("pattern").and_then(|v| v.as_str()) {
                p.to_string()
            } else {
                serde_json::to_string(input).unwrap_or_default()
            }
        }
        _ => serde_json::to_string(input).unwrap_or_default(),
    };
    if raw.chars().count() > 200 {
        let mut truncated: String = raw.chars().take(200).collect();
        truncated.push('…');
        truncated
    } else {
        raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CliKind, LlmProvider};
    use std::collections::HashMap;

    /// Env vars explicitly set on the command via `.env()`.
    fn envs(cmd: &Command) -> HashMap<String, String> {
        cmd.as_std()
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v?.to_str()?.to_string())))
            .collect()
    }

    fn args(cmd: &Command) -> Vec<String> {
        cmd.as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn ctx<'a>(
        settings: &'a Path,
        allowed: &'a [String],
        include_autocoder_tools: bool,
        emit_stream_json: bool,
        resume: Option<&'a str>,
    ) -> BuildContext<'a> {
        BuildContext {
            settings_path: settings,
            allowed_tools: allowed,
            include_autocoder_tools,
            emit_stream_json,
            resume_session_id: resume,
            workspace: Path::new("/tmp"),
            mcp_role: None,
            model: None,
        }
    }

    // 5.3: no model → none of the ANTHROPIC_* vars set.
    #[test]
    fn claude_strategy_no_model_sets_no_anthropic_env() {
        let strat = ClaudeStrategy::new("claude".into(), Vec::new());
        let allowed = vec!["Read".to_string()];
        let mut cmd = strat.build_command(&ctx(
            Path::new("/tmp/s.json"),
            &allowed,
            false,
            false,
            None,
        ));
        strat.apply_model_selection(&mut cmd, None);
        let e = envs(&cmd);
        assert!(!e.contains_key("ANTHROPIC_BASE_URL"));
        assert!(!e.contains_key("ANTHROPIC_AUTH_TOKEN"));
        assert!(!e.contains_key("ANTHROPIC_MODEL"));
    }

    // a003 / task 3.2: a resolved model sets the endpoint + model env
    // (ANTHROPIC_BASE_URL / ANTHROPIC_MODEL) but NO ANTHROPIC_AUTH_TOKEN —
    // the api_key is a credential the subprocess never receives. claude
    // authenticates from its own login. (Supersedes a56's 5.3, which set all
    // three.)
    #[test]
    fn claude_strategy_with_model_sets_endpoint_and_model_but_no_auth_token() {
        let strat = ClaudeStrategy::new("claude".into(), Vec::new());
        let model = ResolvedModel {
            provider: LlmProvider::Anthropic,
            model: "claude-opus-4-8".into(),
            api_base_url: "https://example.invalid/api".into(),
            api_key: "sk-test-sentinel".into(),
        };
        let allowed: Vec<String> = vec![];
        let mut cmd = strat.build_command(&ctx(
            Path::new("/tmp/s.json"),
            &allowed,
            false,
            false,
            None,
        ));
        strat.apply_model_selection(&mut cmd, Some(&model));
        let e = envs(&cmd);
        assert_eq!(
            e.get("ANTHROPIC_BASE_URL").map(String::as_str),
            Some("https://example.invalid/api")
        );
        assert_eq!(e.get("ANTHROPIC_MODEL").map(String::as_str), Some("claude-opus-4-8"));
        // The credential is NEVER set in the subprocess env.
        assert!(
            !e.contains_key("ANTHROPIC_AUTH_TOKEN"),
            "a003: the claude strategy must set no ANTHROPIC_AUTH_TOKEN"
        );
        // Belt-and-braces: the key value appears in NO env entry at all.
        assert!(
            !e.values().any(|v| v.contains("sk-test-sentinel")),
            "a003: the resolved api_key must not appear in any env entry"
        );
    }

    // The claude strategy reproduces the pre-refactor executor streaming
    // invocation exactly (the "byte-identical command" scenario).
    #[test]
    fn claude_strategy_reproduces_streaming_invocation() {
        let strat = ClaudeStrategy::new("claude".into(), Vec::new());
        let allowed = vec!["Read".to_string(), "Write".to_string()];
        let cmd = strat.build_command(&ctx(
            Path::new("/tmp/s.json"),
            &allowed,
            true,
            true,
            None,
        ));
        let combined = build_allowed_tools_value(&allowed, true);
        assert_eq!(cmd.as_std().get_program().to_string_lossy(), "claude");
        assert_eq!(
            args(&cmd),
            vec![
                "--settings".to_string(),
                "/tmp/s.json".into(),
                "--allowedTools".into(),
                combined,
                "--permission-mode".into(),
                "acceptEdits".into(),
                "--verbose".into(),
                "--output-format".into(),
                "stream-json".into(),
            ]
        );
    }

    // The recovery invocation: `--resume` after `acceptEdits`, before the
    // stream-json flags, AND no auto-appended autocoder MCP tools.
    #[test]
    fn claude_strategy_recovery_invocation_has_resume_and_plain_allowed_tools() {
        let strat = ClaudeStrategy::new("claude".into(), Vec::new());
        let allowed = vec!["Read".to_string()];
        let cmd = strat.build_command(&ctx(
            Path::new("/tmp/s.json"),
            &allowed,
            false,
            true,
            Some("sess-123"),
        ));
        let a = args(&cmd);
        let pos = |s: &str| a.iter().position(|x| x == s).expect("arg present");
        assert!(pos("acceptEdits") < pos("--resume"));
        assert!(pos("--resume") < pos("--verbose"));
        assert_eq!(a[pos("--resume") + 1], "sess-123");
        // Plain join — the autocoder MCP tools are NOT appended in recovery.
        assert_eq!(a[pos("--allowedTools") + 1], "Read");
    }

    // Anthropic resolves to the claude strategy.
    #[test]
    fn strategy_for_provider_anthropic_resolves_claude() {
        assert!(strategy_for_provider(LlmProvider::Anthropic, "claude".into(), Vec::new()).is_ok());
    }

    // a60 / task 4.1: the non-Anthropic providers resolve (via a55's
    // `provider → CLI` rule) to a working `OpencodeStrategy` — NOT the
    // pre-a60 "no registered strategy" error — AND it builds an `opencode
    // run` invocation.
    #[test]
    fn strategy_for_provider_non_claude_resolves_opencode() {
        for p in [LlmProvider::OpenAiCompatible, LlmProvider::Ollama] {
            let strat = strategy_for_provider(p, "opencode".into(), Vec::new())
                .expect("non-anthropic provider resolves to the opencode strategy (a60)");
            let allowed = vec!["Read".to_string()];
            let tmp = tempfile::tempdir().unwrap();
            let bctx = BuildContext {
                workspace: tmp.path(),
                ..ctx(Path::new("/tmp/s.json"), &allowed, false, false, None)
            };
            let cmd = strat.build_command(&bctx);
            assert_eq!(cmd.as_std().get_program().to_string_lossy(), "opencode");
            assert_eq!(args(&cmd), vec!["run".to_string()]);
        }
    }

    // a60 / task 4.1: explicit `cli: opencode` (registry override) resolves
    // to the opencode strategy.
    #[test]
    fn strategy_for_cli_opencode_resolves() {
        assert!(strategy_for_cli(CliKind::Opencode, "opencode".into(), Vec::new()).is_ok());
    }

    #[test]
    fn build_allowed_tools_value_appends_mcp_tools_only_when_requested() {
        let allowed = vec!["Read".to_string(), "Edit".to_string()];
        let plain = build_allowed_tools_value(&allowed, false);
        assert_eq!(plain, "Read,Edit");
        let with_mcp = build_allowed_tools_value(&allowed, true);
        assert!(with_mcp.starts_with("Read,Edit,"));
        for tool in crate::mcp_askuser_server::PROVIDED_TOOL_NAMES {
            assert!(
                with_mcp.contains(&crate::mcp_askuser_server::qualified_tool_name(tool)),
                "{tool} must be auto-appended: {with_mcp}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // a60: OpencodeStrategy.
    // -----------------------------------------------------------------------

    fn read_opencode_json(workspace: &Path) -> serde_json::Value {
        let raw = std::fs::read_to_string(workspace.join("opencode.json"))
            .expect("opencode.json was written");
        serde_json::from_str(&raw).expect("opencode.json is valid JSON")
    }

    // a60 / task 4.2: the strategy writes `opencode.json` with the `mcp`
    // block (`type: local`, the MCP-child command, env incl. ORCH_MCP_ROLE)
    // AND writes NO `.mcp.json`.
    #[test]
    fn opencode_strategy_writes_opencode_json_with_mcp_block_and_no_dot_mcp_json() {
        let tmp = tempfile::tempdir().unwrap();
        let allowed = vec!["Read".to_string(), "Glob".to_string(), "Grep".to_string()];
        let bctx = BuildContext {
            workspace: tmp.path(),
            mcp_role: Some("reviewer"),
            ..ctx(Path::new("/tmp/s.json"), &allowed, true, false, None)
        };
        let strat = OpencodeStrategy::new("opencode".into(), Vec::new());
        let _ = strat.build_command(&bctx);

        assert!(
            tmp.path().join("opencode.json").exists(),
            "opencode.json must be written into the workspace"
        );
        assert!(
            !tmp.path().join(".mcp.json").exists(),
            "the opencode strategy must NOT write .mcp.json (that is the claude format)"
        );

        let v = read_opencode_json(tmp.path());
        let server = &v["mcp"][crate::mcp_askuser_server::SERVER_NAME];
        assert_eq!(server["type"], "local");
        assert_eq!(server["enabled"], true);
        let command = server["command"].as_array().expect("command is an array");
        assert_eq!(
            command.last().and_then(|v| v.as_str()),
            Some("mcp-ask-user-server"),
            "MCP child launches the autocoder mcp-ask-user-server subcommand"
        );
        let env = &server["environment"];
        assert_eq!(env[crate::mcp_askuser_server::ENV_ROLE], "reviewer");
        assert_eq!(env[crate::mcp_askuser_server::ENV_CHANGE], "reviewer");
        assert!(
            env[crate::mcp_askuser_server::ENV_WORKSPACE].is_string(),
            "the workspace env var is always written"
        );
    }

    // a60 / task 4.2: with no role, no submission env is advertised (no
    // ORCH_MCP_ROLE / ORCH_MCP_CHANGE).
    #[test]
    fn opencode_strategy_omits_role_env_when_no_role() {
        let tmp = tempfile::tempdir().unwrap();
        let allowed = vec!["Read".to_string()];
        let bctx = BuildContext {
            workspace: tmp.path(),
            mcp_role: None,
            ..ctx(Path::new("/tmp/s.json"), &allowed, true, false, None)
        };
        let strat = OpencodeStrategy::new("opencode".into(), Vec::new());
        let _ = strat.build_command(&bctx);

        let v = read_opencode_json(tmp.path());
        let env = &v["mcp"][crate::mcp_askuser_server::SERVER_NAME]["environment"];
        assert!(env.get(crate::mcp_askuser_server::ENV_ROLE).is_none());
        assert!(env.get(crate::mcp_askuser_server::ENV_CHANGE).is_none());
    }

    // a60 / task 4.3 + a003 / task 3.1: model selection targets the configured
    // provider — `--model <provider>/<model>` + the opencode.json provider
    // entry (model + base URL) — AND sets none of the ANTHROPIC_* env vars.
    // a003: the keyed model's `api_key` is NEVER written into opencode.json
    // (the provider `options` carry the base URL but no `apiKey`), and the key
    // value appears nowhere in the file.
    #[test]
    fn opencode_strategy_model_selection_sets_model_flag_and_provider_no_api_key() {
        let tmp = tempfile::tempdir().unwrap();
        let allowed = vec!["Read".to_string()];
        let model = ResolvedModel {
            provider: LlmProvider::OpenAiCompatible,
            model: "gpt-4o-mini".into(),
            api_base_url: "https://api.example.invalid/v1".into(),
            api_key: "sk-secret-sentinel".into(),
        };
        let bctx = BuildContext {
            workspace: tmp.path(),
            mcp_role: Some("reviewer"),
            model: Some(&model),
            ..ctx(Path::new("/tmp/s.json"), &allowed, true, false, None)
        };
        let strat = OpencodeStrategy::new("opencode".into(), Vec::new());
        let mut cmd = strat.build_command(&bctx);
        strat.apply_model_selection(&mut cmd, Some(&model));

        let a = args(&cmd);
        let pos = a.iter().position(|x| x == "--model").expect("--model present");
        assert_eq!(a[pos + 1], "openai_compatible/gpt-4o-mini");

        let v = read_opencode_json(tmp.path());
        // The MCP, permission, and provider-base-URL blocks are all present.
        assert!(v["mcp"][crate::mcp_askuser_server::SERVER_NAME].is_object());
        assert!(v["permission"].is_object());
        let provider = &v["provider"]["openai_compatible"];
        assert_eq!(provider["options"]["baseURL"], "https://api.example.invalid/v1");
        assert!(
            provider["options"].get("apiKey").is_none(),
            "a003: the resolved api_key must NOT be written into opencode.json"
        );
        assert!(
            provider["models"]["gpt-4o-mini"].is_object(),
            "the resolved model is registered under the provider"
        );
        // The key value appears nowhere in the serialized config.
        let raw = std::fs::read_to_string(tmp.path().join("opencode.json")).unwrap();
        assert!(
            !raw.contains("sk-secret-sentinel"),
            "a003: the resolved api_key must not appear anywhere in opencode.json"
        );

        let e = envs(&cmd);
        assert!(!e.contains_key("ANTHROPIC_BASE_URL"));
        assert!(!e.contains_key("ANTHROPIC_AUTH_TOKEN"));
        assert!(!e.contains_key("ANTHROPIC_MODEL"));
    }

    // a60 / task 4.3: an Ollama model (no api key) omits the apiKey option.
    #[test]
    fn opencode_strategy_ollama_provider_omits_api_key() {
        let tmp = tempfile::tempdir().unwrap();
        let allowed = vec!["Read".to_string()];
        let model = ResolvedModel {
            provider: LlmProvider::Ollama,
            model: "qwen2.5-coder".into(),
            api_base_url: "http://localhost:11434".into(),
            api_key: String::new(),
        };
        let bctx = BuildContext {
            workspace: tmp.path(),
            model: Some(&model),
            ..ctx(Path::new("/tmp/s.json"), &allowed, true, false, None)
        };
        let strat = OpencodeStrategy::new("opencode".into(), Vec::new());
        let mut cmd = strat.build_command(&bctx);
        strat.apply_model_selection(&mut cmd, Some(&model));

        let a = args(&cmd);
        let pos = a.iter().position(|x| x == "--model").expect("--model present");
        assert_eq!(a[pos + 1], "ollama/qwen2.5-coder");

        let v = read_opencode_json(tmp.path());
        let options = &v["provider"]["ollama"]["options"];
        assert_eq!(options["baseURL"], "http://localhost:11434");
        assert!(
            options.get("apiKey").is_none(),
            "ollama does not authenticate; apiKey must be omitted"
        );
    }

    // a60 / task 4.4: a read-only role's Write/Edit/Bash are denied via the
    // generated permission config; the role's MCP tool is still exposed.
    #[test]
    fn opencode_strategy_readonly_denies_write_edit_bash() {
        let tmp = tempfile::tempdir().unwrap();
        let allowed = vec!["Read".to_string(), "Glob".to_string(), "Grep".to_string()];
        let bctx = BuildContext {
            workspace: tmp.path(),
            mcp_role: Some("reviewer"),
            ..ctx(Path::new("/tmp/s.json"), &allowed, true, false, None)
        };
        let strat = OpencodeStrategy::new("opencode".into(), Vec::new());
        let _ = strat.build_command(&bctx);

        let v = read_opencode_json(tmp.path());
        let perm = &v["permission"];
        assert_eq!(perm["edit"], "deny", "Write/Edit denied for a read-only role");
        assert_eq!(perm["bash"], "deny", "Bash denied for a read-only role");
        assert_eq!(perm["webfetch"], "deny");
        // The role's submission tool stays reachable via the mcp block.
        assert_eq!(
            v["mcp"][crate::mcp_askuser_server::SERVER_NAME]["environment"]
                [crate::mcp_askuser_server::ENV_ROLE],
            "reviewer"
        );
    }

    // a60 / task 4.4 (converse): a write-enabled sandbox allows edit + bash.
    #[test]
    fn opencode_strategy_write_sandbox_allows_edit_and_bash() {
        let tmp = tempfile::tempdir().unwrap();
        let allowed = crate::config::default_allowed_tools();
        let bctx = BuildContext {
            workspace: tmp.path(),
            ..ctx(Path::new("/tmp/s.json"), &allowed, true, false, None)
        };
        let strat = OpencodeStrategy::new("opencode".into(), Vec::new());
        let _ = strat.build_command(&bctx);

        let v = read_opencode_json(tmp.path());
        assert_eq!(v["permission"]["edit"], "allow");
        assert_eq!(v["permission"]["bash"], "allow");
    }

    // a60 / task 4.5: an opencode role runs through `agentic_run` in capture
    // mode — stdout/stderr read at exit, NO streaming-JSON parse (no
    // final_answer / session_id / structured log).
    #[tokio::test]
    async fn opencode_role_runs_through_agentic_run_in_capture_mode() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        // Stub `opencode`: drain stdin (the piped prompt), print a line,
        // exit 0. Stands in for the real binary so the capture path runs.
        let stub = tmp.path().join("opencode_stub.sh");
        std::fs::write(&stub, "#!/bin/sh\ncat >/dev/null\necho 'opencode stub done'\n").unwrap();
        let mut perms = std::fs::metadata(&stub).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&stub, perms).unwrap();

        let strat = OpencodeStrategy::new(stub.to_string_lossy().into_owned(), Vec::new());
        let outcome = agentic_run(AgenticRunOpts {
            workspace: tmp.path(),
            change: "reviewer",
            strategy: &strat,
            prompt: "review this change",
            sandbox: SandboxConfig {
                allowed_tools: vec!["Read".to_string()],
                disallowed_bash_patterns: Vec::new(),
                disallowed_read_paths: Vec::new(),
                deny_writes: true,
            },
            model: None,
            output_mode: OutputMode::Capture,
            timeout: std::time::Duration::from_secs(30),
            paths: None,
            settings_dir: Some(tmp.path()),
            include_autocoder_tools: true,
            emit_stream_json_in_capture: false,
            resume_session_id: None,
            track_subprocess_marker: false,
            etxtbsy_retry_spawn: false,
            // Unenforced: this test exercises the inner capture path, not the
            // OS layer (no mechanism is runnable in CI).
            os_sandbox: crate::sandbox::RunSandbox::default(),
        })
        .await
        .expect("agentic_run completes for the opencode stub");

        assert!(!outcome.timed_out);
        assert!(
            outcome.stdout.contains("opencode stub done"),
            "capture mode reads stdout at exit: {:?}",
            outcome.stdout
        );
        assert!(
            outcome.final_answer.is_none(),
            "capture mode does NOT parse a streaming-JSON final_answer"
        );
        assert!(
            outcome.session_id.is_none(),
            "capture mode does NOT parse a streaming-JSON session_id"
        );
        assert!(
            !outcome.streamed_log,
            "capture mode does NOT write the streaming structured log"
        );
        // The strategy wrote opencode.json (not .mcp.json) for the run.
        assert!(tmp.path().join("opencode.json").exists());
        assert!(!tmp.path().join(".mcp.json").exists());
    }

    // -----------------------------------------------------------------------
    // a003: credentials never reach the model.
    // -----------------------------------------------------------------------

    /// The sentinel a strategy has no legitimate reason to ever emit.
    const KEY_SENTINEL: &str = "SENTINEL-API-KEY-MUST-NOT-LEAK-9f3c";

    /// Build a keyed [`ResolvedModel`] for `provider` carrying [`KEY_SENTINEL`].
    fn sentinel_model(provider: LlmProvider) -> ResolvedModel {
        ResolvedModel {
            provider,
            model: "the-model".into(),
            api_base_url: "https://api.example.invalid/v1".into(),
            api_key: KEY_SENTINEL.into(),
        }
    }

    /// Drive one strategy with the keyed model AND assert the sentinel appears
    /// in NO subprocess env entry AND in NO file the strategy wrote into the
    /// workspace.
    fn assert_strategy_leaks_no_key(strat: &dyn CliStrategy, provider: LlmProvider) {
        let tmp = tempfile::tempdir().unwrap();
        let model = sentinel_model(provider);
        let allowed = vec!["Read".to_string()];
        let bctx = BuildContext {
            workspace: tmp.path(),
            mcp_role: Some("reviewer"),
            model: Some(&model),
            ..ctx(Path::new("/tmp/s.json"), &allowed, true, false, None)
        };
        let mut cmd = strat.build_command(&bctx);
        strat.apply_model_selection(&mut cmd, Some(&model));

        // No subprocess env entry carries the key.
        for (k, v) in envs(&cmd) {
            assert!(
                !v.contains(KEY_SENTINEL),
                "env `{k}` leaked the api_key for provider {provider:?}"
            );
        }
        // No file the strategy wrote into the workspace carries the key.
        for entry in std::fs::read_dir(tmp.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                let raw = std::fs::read_to_string(&path).unwrap_or_default();
                assert!(
                    !raw.contains(KEY_SENTINEL),
                    "workspace file {} leaked the api_key for provider {provider:?}",
                    path.display()
                );
            }
        }
    }

    // a003 / task 3.3: across EVERY registered CliStrategy, no file written into
    // the workspace AND no subprocess env entry contains the resolved api_key.
    #[test]
    fn no_strategy_leaks_api_key_to_file_or_env() {
        assert_strategy_leaks_no_key(
            &ClaudeStrategy::new("claude".into(), Vec::new()),
            LlmProvider::Anthropic,
        );
        assert_strategy_leaks_no_key(
            &OpencodeStrategy::new("opencode".into(), Vec::new()),
            LlmProvider::OpenAiCompatible,
        );
    }

    // a003 / task 3.5: a CLI role configured with an api_key produces exactly
    // one startup WARN (the key is unused for CLI roles) AND the strategy
    // ignores the key. A role with no key produces no WARN.
    #[test]
    fn cli_role_with_key_warns_once_and_strategy_ignores_it() {
        // With a key: exactly one WARN, naming the role AND the unused-key reason.
        let role = "executor.change_internal_contradiction_check_llm";
        let msg = cli_role_unused_key_warning(role, true)
            .expect("a keyed CLI role must produce exactly one WARN");
        assert!(msg.contains(role), "the WARN names the role: {msg}");
        assert!(
            msg.to_ascii_lowercase().contains("unused"),
            "the WARN explains the key is unused: {msg}"
        );
        assert!(msg.contains("api_key"), "the WARN names the field: {msg}");

        // With no key: no WARN.
        assert!(
            cli_role_unused_key_warning(role, false).is_none(),
            "a role with no configured key must not warn"
        );

        // The strategy ignores the key even when the model carries one: the
        // claude strategy sets no ANTHROPIC_AUTH_TOKEN and leaks nothing.
        let strat = ClaudeStrategy::new("claude".into(), Vec::new());
        let model = sentinel_model(LlmProvider::Anthropic);
        let allowed: Vec<String> = vec![];
        let mut cmd =
            strat.build_command(&ctx(Path::new("/tmp/s.json"), &allowed, false, false, None));
        strat.apply_model_selection(&mut cmd, Some(&model));
        let e = envs(&cmd);
        assert!(!e.contains_key("ANTHROPIC_AUTH_TOKEN"));
        assert!(!e.values().any(|v| v.contains(KEY_SENTINEL)));
    }
}
