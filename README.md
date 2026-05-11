# OpenSpec CI/CD Orchestrator

The OpenSpec CI/CD Orchestrator is an autonomous server daemon designed to read OpenSpec implementation proposals, execute an AI implementation agent (like Gemini or Claude) via a CLI, and create Pull Requests. This effectively creates an "Autonomous AI Software Factory" using OpenSpec as the standardized work order system.

## Architecture

The Orchestrator operates on a "Serial Queue" model per repository to manage dependent features safely and avoid Git merge conflicts.

1. **Queue Engine**: Polling mechanism that monitors the `openspec/changes/` directory for ready changes and reads a YAML configuration for multiple repositories.
2. **Workspace Manager**: Clones or pulls target repositories into `/tmp/workspaces/<repo-name>` to isolate agent execution.
3. **Agent Subprocess Runner**: Executes the AI agent CLI (e.g. `gemini-cli /opsx:apply <change-name>`) as a blocking subprocess.
4. **Git Workflow**: Automates `git checkout`, creating a new agent branch (`agent-q` or custom), committing the AI's changes, and opening a monolithic PR at the end of the polling cycle.
5. **Reviewer Integration**: An automated post-commit review step invoking a second agent (like Grok or MiMo) to assess code quality before human review.
6. **Recovery System**: A robust `rewind` command that cleans up corrupted branches and unarchives changes back into the active queue when a failure occurs or a PR is rejected.

---

## Configuration

The Orchestrator accepts a `config.yaml` file specifying the repositories to watch, their polling intervals, and custom branch names. Multiple repositories are first-class: the orchestrator spawns one polling task per `repositories[]` entry, each on its own interval.

Copy the example file to get started:
```bash
cp config.example.yaml config.yaml
```

**Example `config.yaml` (multi-repo):**
```yaml
repositories:
  - url: "git@github.com:my-org/auth-service.git"
    base_branch: "main"
    agent_branch: "agent-q"
    poll_interval_sec: 300

  - url: "git@github.com:my-org/web-dashboard.git"
    base_branch: "dev"
    agent_branch: "agent-q"
    poll_interval_sec: 3600

executor:
  kind: claude_cli           # currently the only supported backend
  command: claude
  timeout_secs: 1800

github:
  token_env: GITHUB_TOKEN    # env var holding the PAT used for PR creation
```

### Workspace Path Derivation

If a repository entry omits `local_path`, the workspace path is derived deterministically from the URL:

1. Strip the protocol prefix (`git@`, `ssh://`, `https://`, `http://`).
2. Strip a trailing `.git`.
3. Replace any character that is not ASCII alphanumeric, `_`, or `-` with `_`.
4. Prepend `/tmp/workspaces/`.

This means `git@github.com:owner/repo.git` and `https://github.com/owner/repo.git` both map to `/tmp/workspaces/github_com_owner_repo`. At startup, the orchestrator runs a collision check: if two configured repositories resolve to the same workspace path (whether by derivation or by explicit `local_path`), the process exits non-zero before spawning any polling tasks. Set `local_path` explicitly to disambiguate.

## ChatOps Escalation

When the optional `slack:` config block is present, the orchestrator routes ambiguous agent outcomes (executor returning `AskUser`) to a human via Slack thread replies, persists the conversation state to disk, and resumes implementation on the next iteration when an answer arrives.

### Configuring Slack

```yaml
slack:
  bot_token_env: SLACK_BOT_TOKEN        # env var containing your xoxb-... bot token
  default_channel_id: C0123456789       # fallback channel id (use the Slack channel ID, not the name)
```

Per-repo override:

```yaml
repositories:
  - url: "git@github.com:my-org/auth-service.git"
    # ...
    slack_channel_id: C0AUTH_CHANNEL    # this repo posts to a different channel
```

### Required Slack bot scopes

The Slack app's bot token must have at least these OAuth scopes:

- `chat:write` — post the escalation message into the channel.
- `channels:history` — read thread replies on public channels.
- `channels:read` — list channels (for token validation).

The orchestrator does NOT need `users:read` or any user-level scopes; reply attribution is by Slack user id only.

### What gets posted

When an executor returns `AskUser { question, resume_handle }`, the orchestrator posts to the resolved channel:

```
❓ `<change-name>`: <question text>
```

The resulting Slack message's thread timestamp + the executor's opaque resume handle are persisted to `<workspace>/openspec/changes/<change-name>/.question.json`. The agent's `.in-progress` lock is removed, so the change moves from "in flight" to "waiting on human."

### How reply detection works

On every polling iteration, BEFORE the orchestrator considers pending changes for that repository, it:

1. Calls `queue::list_waiting(workspace)` to find all `.question.json`-bearing changes.
2. For each, GETs `conversations.replies` on the tracked thread.
3. The **first message** that has no `bot_id` field AND whose `user` differs from the orchestrator's own bot user id is treated as the human's answer.
4. The orchestrator writes `.answer.json`, deletes `.question.json`, calls `executor.resume(handle, answer)`, and handles the new outcome like a fresh run (commit + archive on `Completed`, escalate again on a second `AskUser`, log + revert to pending on `Failed`).

### Same-repo queue blocking

A change waiting on a human answer in repository X blocks ALL pending-change processing for repository X. This preserves the architecture's serial-queue invariant: when change A asks a question, change B (which may depend on A's restructuring) is NOT processed until A is resolved. Cross-repo polling tasks are independent — repository Y continues to be serviced.

### Operator escape hatches for a stuck waiting change

If a Slack reply never arrives (operator on vacation, lost ping, changed mind), the orchestrator does not time out — it waits indefinitely. Three operator-controlled ways to unblock:

1. **Reply in Slack** — the original Slack thread is still tracked. Send any non-bot message in that thread; the next polling iteration resumes the change.
2. **Manually delete `.question.json`** — this reverts the change to pending state. The next iteration will re-run it from scratch (without the answer). Useful when the question was a false positive or the change should restart.
3. **`orchestrator rewind <change>`** — full reset: deletes the agent branch, unarchives if needed, clears all `.question.json` / `.answer.json` markers via the rewind path.

### `.question.json` and `.answer.json` as workspace artifacts

These files are written by the orchestrator into the workspace alongside the change's `proposal.md`. They are safe to inspect (plain JSON) but unsafe to modify by hand — atomic writes via temp-file-then-rename mean they're consistent on disk, but the orchestrator's state machine assumes it owns their lifecycle. When a change is archived, the directory move takes the marker files with it; they're not deleted separately.

---

## Code Review

When the optional `reviewer:` config block is present and `enabled: true`, every PR opened by the orchestrator includes a structured AI-generated code-quality review under a `## Code Review` heading in the PR body. A `Block` verdict additionally causes the PR to be created as a draft.

### Scope

The reviewer's job is **code quality only**: security (injection, auth, secrets), error handling, naming/style/idioms, dead code, obvious bugs. It explicitly does **not** assess whether the diff implements the spec — that is a separate concern handled by the (future) verifier change. The default prompt template (`prompts/code-review-default.md`) enforces this scope statement at the top.

### Configuring the reviewer

```yaml
reviewer:
  enabled: true
  provider: anthropic               # or `openai_compatible`
  model: claude-sonnet-4-6
  api_key_env: ANTHROPIC_API_KEY    # env var containing the API token
  api_base_url: https://api.anthropic.com   # optional; provider default if omitted
  prompt_template_path: ./prompts/code-review-default.md  # optional; built-in default if omitted
```

The `openai_compatible` provider works with any endpoint that speaks the OpenAI `/chat/completions` API — Grok, OpenRouter, local Ollama, etc. Point `api_base_url` at the endpoint and provide a matching token via `api_key_env`.

### Verdict semantics

| Verdict     | PR state  | Meaning                                                                   |
|-------------|-----------|---------------------------------------------------------------------------|
| `Pass`      | non-draft | No concerns above style nits.                                              |
| `Concerns`  | non-draft | Issues warrant discussion but the diff is mergeable.                       |
| `Block`     | **draft** | At least one issue would cause real harm if merged.                        |

If the LLM's response cannot be parsed for a verdict, the orchestrator defaults to `Concerns` and prepends a parse-failure note to the report. If the API call itself errors (network, auth, rate limit), the orchestrator logs the error and ships the PR anyway with `(reviewer failed: <reason>)` in the `## Code Review` section. **A failed reviewer never blocks PR creation.**

### Block-verdict enforcement (recommended)

The orchestrator does its part by creating the PR as a draft. To make `Block` actually gate merge, configure a branch-protection rule on the PR target branch that **requires PRs not be draft**. Without that rule, anyone with write access can flip the draft state and merge.

On hosts that don't support drafts (some private GHE configurations, certain repo types), the orchestrator falls back automatically: it retries the PR creation with `draft: false` and applies a `do-not-merge` label via the issues-labels endpoint. Configure your branch protection to require the absence of that label as the fallback gate.

### Custom prompt templates

If the default template doesn't match your project's style, override it via `reviewer.prompt_template_path`. Custom templates are **user-owned** — the project does not enforce scope on overrides, so if you want to expand the reviewer to additional dimensions (spec compliance, style guide, etc.), you can. The template must include the two substitution variables `{{diff}}` and `{{change_summary}}` and must instruct the model to begin its response with a line of the form `VERDICT: Pass`, `VERDICT: Concerns`, or `VERDICT: Block`.

---

## CLI Usage

The orchestrator provides a simple CLI with two main commands:

### `run`
Starts the asynchronous polling daemon. It will infinitely poll all configured repositories.

```bash
# Run with default config.yaml
cargo run -- run

# Run with a custom config file
cargo run -- run --config /path/to/my-config.yaml
```

### `rewind`
A recovery command to use when an agent has produced bad work or you want to throw away the in-flight agent branch and re-run one or more archived changes. Rewind:

1. Deletes the local agent branch (always).
2. Deletes the remote agent branch (only with `--hard`).
3. Unarchives each named change so the polling daemon picks it up again.

```bash
# Soft rewind (single-repo config): prompt for confirmation, then delete
# the local agent branch and unarchive one change.
orchestrator rewind my-broken-change --config config.yaml

# Hard rewind: skip the prompt, delete local AND remote agent branch,
# then unarchive two changes.
orchestrator rewind change-A change-B --config config.yaml --hard

# Multi-repo config: --repo is REQUIRED. The selector matches either the
# full URL or the short-name (basename minus .git).
orchestrator rewind my-change --config config.yaml --repo my-repo
```

**Soft vs hard semantics:**

| Mode   | Confirmation prompt | Local agent branch | Remote agent branch                       |
|--------|--------------------|--------------------|-------------------------------------------|
| soft   | y/N, defaults no   | deleted            | left intact                                |
| `--hard` | skipped          | deleted            | deleted (failures logged but non-blocking) |

The confirmation prompt for soft rewind looks like:

```
This will delete branch 'agent-q' (local) and unarchive 1 change(s) (my-broken-change). Proceed? [y/N]
```

Bare Enter, `n`, or any input other than `y`/`Y` declines and exits without modifying any state.

**`--repo` selector:**

With **one** configured repository, `--repo` is optional and defaults to that repo.

With **two or more** configured repositories, `--repo` is required. The orchestrator matches the selector against each repository's full URL (exact equality) AND against the URL's short-name (basename with any trailing `.git` stripped). Zero matches or multiple matches exit non-zero with a clear error listing the available selectors.

**Unarchiving multiple changes:**

If you pass multiple change names and one of them fails to unarchive (typo, no matching archive entry, destination collision), the remaining names are still attempted. The process exits non-zero at the end with a summary naming both the succeeded and failed changes.

**"I rewound the wrong change":**

Archived directories are **not** deleted by archive — they are renamed under `openspec/changes/archive/<YYYY-MM-DD>-<name>/`. If you accidentally rewind a change and want to put it back, you can move the directory back into the archive yourself (the canonical date-prefix format is preserved by the orchestrator's `archive` step, so a manual `mv` restores the queue's view of state).

---

## Deployment Guide

For long-term production use, it is highly recommended to run the Orchestrator as a persistent systemd service on a dedicated Linux server rather than running it locally or via Cron (since the daemon handles its own polling loop internally).

### 1. Build the Release Binary
```bash
cargo build --release
sudo cp target/release/orchestrator /usr/local/bin/openspec-orchestrator
```

### 2. Set up Systemd Service
Create a new file at `/etc/systemd/system/openspec-orchestrator.service`:

```ini
[Unit]
Description=OpenSpec Autonomous CI/CD Orchestrator
After=network.target

[Service]
Type=simple
User=orchestrator-user
# Ensure this directory contains your config.yaml
WorkingDirectory=/opt/openspec-orchestrator
# Pass environment variables for your AI Agents
Environment="GEMINI_API_KEY=your_key_here"
Environment="ANTHROPIC_API_KEY=your_key_here"
# The git client will use the user's ~/.ssh keys for repo access
ExecStart=/usr/local/bin/openspec-orchestrator run --config config.yaml
Restart=on-failure
RestartSec=10

[Install]
WantedBy=multi-user.target
```

### 3. Start and Enable
```bash
sudo systemctl daemon-reload
sudo systemctl enable openspec-orchestrator
sudo systemctl start openspec-orchestrator
```

### Updating the Daemon
To update the orchestrator, pull the latest code, rebuild the release binary, copy it to `/usr/local/bin`, and restart the service: `sudo systemctl restart openspec-orchestrator`.

---

## AI Security & Guardrails

Running autonomous agents with push access to your repositories introduces unique security challenges. Please adhere to the following best practices:

### 1. Credential Scoping
Never give the orchestrator a Personal Access Token (PAT) or SSH key with admin access to your organization. Provide it with **scoped, write-only access** strictly limited to the repositories defined in `config.yaml`.

### 2. Branch Protection
Protect your `main` and `dev` branches. The orchestrator should **never** be allowed to push directly to protected branches. It must be constrained to pushing to its designated `agent_branch` and opening Pull Requests for human review.

### 3. The "Self-Modifying AI" Risk
If you configure the Orchestrator to watch its own repository (e.g., `cicd-impl-agents`), you introduce the risk of the AI modifying its own source code. 
*   **The Danger:** If an agent gets stuck in a loop of failing tests, a "lazy" LLM might attempt to solve the problem by deleting the tests, modifying the OpenSpec schema, or altering its own system prompts within the orchestrator codebase.
*   **The Mitigation:** Always require Human + Reviewer Agent approval for PRs merged into the orchestrator's own repository. Never configure the orchestrator to automatically merge its own PRs without human intervention.

### 4. Workspace Isolation
The orchestrator clones repositories into `/tmp/workspaces/`. Ensure this partition has sufficient disk space and is regularly cleared of orphaned locks (`.in-progress`) upon system restarts. Do not run the orchestrator with root privileges.

---
*Note: This documentation is maintained per the `project-documentation` OpenSpec rule. Any new capabilities or operational shifts must be updated here.*