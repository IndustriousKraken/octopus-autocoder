# Deployment

For production, run autocoder as a systemd service on a dedicated Linux host. The daemon polls on its own — do not wrap it in a cron job.

## Recommended: install from a binary release

For most operators, the [Quick install](../README.md#quick-install) one-liner is the right path. It downloads a pre-built binary from the [GitHub Releases](https://github.com/IndustriousKraken/openspec-autocoder/releases) page (per tag, for `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, and `aarch64-apple-darwin`), verifies its SHA-256, and then runs `autocoder install` to set up the systemd service and configuration. Releases are versioned with SemVer tags (`vX.Y.Z`); dash-suffixed tags such as `vX.Y.Z-rc1` are pre-releases that the installer skips by default. The rest of this section covers the manual / source-build path for operators who specifically want to avoid downloaded binaries.

## 1. Install the binary

```bash
cargo build --release
sudo cp target/release/autocoder /usr/local/bin/autocoder
```

## 2. Create a deploy user and authenticate Claude Code

```bash
sudo useradd -m -s /bin/bash autocoder
sudo -u autocoder -i                            # become the deploy user
claude auth login                                # interactive Anthropic OAuth
git config --global user.email "autocoder@$(hostname)"
git config --global user.name "autocoder"
exit                                             # back to your admin shell

# Install openspec so the executor can generate richer prompts via
# `openspec instructions apply`. Without it the daemon falls back to
# raw markdown concatenation which gives the agent less guidance.
sudo -u autocoder npm install -g @fission-ai/openspec
sudo -u autocoder openspec --version             # verify
```

The Claude credentials now live at `/home/autocoder/.claude/`. The git config writes to `/home/autocoder/.gitconfig` and is required — autocoder's commit step fails without an author identity. Both survive restarts as long as the systemd unit runs as the same user.

(If `npm` isn't on the autocoder user's `$PATH`, install Node.js first via your distro's package manager or `nvm`. The exact openspec install command may vary; check the openspec project for the current recommendation.)

After installing the openspec CLI, run `openspec config profile` once on this host and enable the `Sync specs` workflow:

```bash
sudo -u autocoder openspec config profile
```

This launches an interactive picker. Choose **Delivery: Both (skills + commands)** and at minimum tick **Sync specs** in the workflow list. Then in each project the daemon operates on, refresh the project's openspec install so the new workflows take effect:

```bash
sudo -u autocoder bash -c 'cd /var/cache/autocoder/workspaces/<sanitized-url> && openspec update'
```

autocoder's archive step shells out to `openspec archive`, which performs both the file move AND the merge of change deltas into canonical capability specs — but the merge step is only available when `sync` is enabled in the openspec profile. Without it, `openspec archive` will move the change directory but won't update canonical specs; autocoder iterations succeed but drift accumulates in `openspec/specs/`. To reconcile drift after the fact (e.g. for repos with pre-existing drift, or after onboarding a repo from a host that didn't have `sync` enabled), see the companion `rebuild-canonical-specs-from-archive` change.

## 3. Set up SSH for the autocoder user

Required for `config.yaml` repositories using SSH URLs (`git@github.com:...`), which is the recommended form for multi-owner setups. The autocoder user needs an SSH key tied to a GitHub identity with access to exactly the configured repositories — no more.

Generate the keypair and pre-accept github.com's host key:

```bash
# Generate a passphrase-less key for the autocoder user. The outer single
# quotes are required so `-N ""` survives sudo's argument handling.
sudo -u autocoder bash -c 'mkdir -p ~/.ssh && ssh-keygen -t ed25519 -C "autocoder@$(hostname)" -f ~/.ssh/id_ed25519 -N ""'

# Pre-accept github.com's host key so the daemon never hits an interactive prompt.
sudo -u autocoder bash -c 'ssh-keyscan github.com >> ~/.ssh/known_hosts && chmod 600 ~/.ssh/known_hosts'

# Print the public key to register with GitHub.
sudo -u autocoder cat /home/autocoder/.ssh/id_ed25519.pub
```

Then register the public key against a GitHub identity. **Pick one of the three options below** based on your security posture:

### Option A — Machine user (recommended for orgs with real users)

Create a dedicated GitHub account (e.g. `<your-handle>-autocoder`) that exists only to be autocoder. Add it as a member of a team in each org with access to only the repositories in `config.yaml`, then register the SSH key on the machine user's account (*Settings → SSH and GPG keys → New SSH key*).

Required team-grant permission level:

- **Read** if you use [Fork-and-PR mode](SECURITY.md#7-fork-and-pr-workflow-recommended-for-org-repos) (recommended). The machine user only reads upstream and pushes to its own fork.
- **Write** if you use direct-push mode (no `github.fork_owner` set). The machine user pushes the agent branch directly to upstream.

Mint the PATs you set in `config.yaml`'s `github.owner_tokens` from the machine user too — same scoping principle: the credential's authority matches autocoder's job. A full compromise of the autocoder host then gives the attacker exactly the access you granted that user and nothing more.

GitHub's terms of service permit machine users for automation. The account is free.

### Option B — Per-repo deploy keys (works without a separate identity)

Add the same public key as a deploy key on each repo: *Repo settings → Deploy keys → Add deploy key*, with **"Allow write access"** checked so autocoder can push the agent branch.

Caveat: GitHub enforces that any given public key can be registered as a deploy key on **exactly one repo** across the platform. If autocoder manages N repos, you need N keypairs in `~autocoder/.ssh/` plus a `~/.ssh/config` with per-host routing — e.g.:

```
Host github.com-org-a-repo-1
  HostName github.com
  IdentityFile ~/.ssh/id_ed25519_org_a_repo_1
  IdentitiesOnly yes
```

Then the `config.yaml` URL becomes `git@github.com-org-a-repo-1:org-a/repo-1.git`. Manageable up to a handful of repos; tedious past that.

### Option C — Personal-account key (small personal-repo setups only)

Register the key under your own `Settings → SSH and GPG keys → New SSH key`. The autocoder daemon will then act as you for all git operations, with whatever permissions you have. **Do not use this for organization repos with real users** — a compromised autocoder host can `git push` anywhere you can. Acceptable only for solo developers managing their own personal repos.

### Verify

```bash
sudo -u autocoder ssh -T git@github.com
# Expected: "Hi <user>! You've successfully authenticated, but GitHub does not provide shell access."
```

`<user>` will be whichever identity you registered the key under (the machine user, your own account, or — for deploy keys — empty since deploy keys don't have a user identity).

## 4. Stage the working directory

```bash
sudo mkdir -p /home/autocoder/autocoder
sudo cp config.example.yaml /home/autocoder/autocoder/config.yaml
sudo chown -R autocoder:autocoder /home/autocoder/autocoder
sudo -u autocoder $EDITOR /home/autocoder/autocoder/config.yaml   # edit repo URLs, and inline secrets if you chose that path
sudo chmod 600 /home/autocoder/autocoder/config.yaml              # restrictive perms regardless of secret path
```

## 5. Set up the systemd service

Pick one of the two secret-delivery paths below depending on what you put in your `config.yaml` (see [Secrets in `config.yaml`](SECURITY.md#5-secrets-in-configyaml-inline-vs-env-var)).

### Path A — inline secrets (recommended for single-host deployments)

With secrets inline in `config.yaml` (`github.token`, `reviewer.api_key`, `chatops.slack.bot_token`), the unit needs no env vars. Create `/etc/systemd/system/autocoder.service`:

```ini
[Unit]
Description=autocoder — autonomous OpenSpec implementation daemon
After=network.target

[Service]
Type=simple
User=autocoder
WorkingDirectory=/home/autocoder/autocoder

# PATH must include the directories containing `claude` and `openspec` — both
# are invoked by name. systemd does not inherit the operator's interactive
# PATH. `which openspec claude` as the deploy user is the authoritative check.
Environment="PATH=/usr/local/bin:/usr/bin:/bin"

ExecStart=/usr/local/bin/autocoder run --config /home/autocoder/autocoder/config.yaml
Restart=on-failure
RestartSec=60

[Install]
WantedBy=multi-user.target
```

`openspec` must be on autocoder's PATH. The daemon runs `openspec --version` at startup and exits non-zero with a clear stderr message if the binary is missing. Confirm with `sudo -u autocoder which openspec`. The per-change run log at `<logs_dir>/runs/<repo>/<change>.log` (typically `/var/log/autocoder/runs/<repo>/<change>.log` under systemd) records the prompt sent to Claude under a `=== PROMPT (n bytes) ===` header for inspection.

### Path B — env-var secrets (multi-user hosts, classical production pattern)

With `*_env` fields in `config.yaml` (no inline secrets), add an `EnvironmentFile=` directive pointing at a separate, root-owned env file:

```ini
[Unit]
Description=autocoder — autonomous OpenSpec implementation daemon
After=network.target

[Service]
Type=simple
User=autocoder
WorkingDirectory=/home/autocoder/autocoder

# PATH must include the directories containing `claude` and `openspec`.
# See Path A above for the rationale.
Environment="PATH=/usr/local/bin:/usr/bin:/bin"

# Required only if your config.yaml uses *_env fields (env-var secret path).
EnvironmentFile=/etc/autocoder.env

ExecStart=/usr/local/bin/autocoder run --config /home/autocoder/autocoder/config.yaml
Restart=on-failure
RestartSec=60

[Install]
WantedBy=multi-user.target
```

Create `/etc/autocoder.env` (mode `0600`, owned by root):

```
# Single-owner setups: a single PAT named by `github.token_env` in config.yaml.
GITHUB_TOKEN=ghp_yourtokenhere

# Multi-owner setups (see "Multiple GitHub Tokens" above): one PAT per owner.
# Uncomment and adjust to match the env var names referenced from
# `github.owner_tokens:` in config.yaml. GITHUB_TOKEN can be omitted if
# every configured repository's owner has an explicit route.
# PERSONAL_GH_TOKEN=github_pat_xxx_personal
# ORG_A_GH_TOKEN=github_pat_xxx_org_a
# ORG_B_GH_TOKEN=github_pat_xxx_org_b

# Optional, only if the matching config block is enabled and uses *_env:
# ANTHROPIC_API_KEY=...
# SLACK_BOT_TOKEN=xoxb-...        # chatops.provider: slack
# DISCORD_BOT_TOKEN=...           # chatops.provider: discord (EXPERIMENTAL)
# TEAMS_CLIENT_SECRET=...         # chatops.provider: teams (EXPERIMENTAL)
# MATTERMOST_TOKEN=...            # chatops.provider: mattermost (EXPERIMENTAL)
# MATRIX_ACCESS_TOKEN=...         # chatops.provider: matrix (EXPERIMENTAL)
```

The two paths can be mixed per-secret — e.g. inline `github.token` alongside `reviewer.api_key_env: ANTHROPIC_API_KEY` — in which case the unit needs `EnvironmentFile=` and the env file carries only the env-var-sourced secrets.

## 6. Start it

```bash
sudo systemctl daemon-reload
sudo systemctl enable autocoder
sudo systemctl start autocoder
sudo journalctl -u autocoder -f      # tail logs
```

## Applying config changes without a restart

Edit `config.yaml`, then run:

```bash
sudo -u autocoder autocoder reload
```

The `autocoder reload` subcommand connects to the daemon's control socket at `<runtime_dir>/control.sock` (typically `/run/autocoder/control.sock` under systemd, or `${XDG_RUNTIME_DIR}/autocoder/control.sock` in dev mode). That socket is created on startup with mode `0600` and is owned by the user the daemon runs as (the `autocoder` user in this guide), so any reload command must run as the same user — `sudo -u autocoder` is the standard invocation. The daemon re-reads `config.yaml` from the path it was launched with, validates it, and hot-applies the `github`, `reviewer`, `chatops`, and `repositories` sections at the next iteration boundary for each repo. Only changes to `executor:` are not hot-applied; the response names that under `requires_restart` so you know it still needs `systemctl restart autocoder`. See [Runtime control: live config reload](OPERATIONS.md#runtime-control-live-config-reload) above for the full response shape and validation-rejection semantics.

## Upgrading

Build the new release, copy the binary, restart the unit:

```bash
cd /path/to/cicd-impl-agents
git pull
cargo build --release
sudo cp target/release/autocoder /usr/local/bin/autocoder
sudo systemctl restart autocoder
```

If you were on an older version that installed under `/usr/local/bin/openspec-orchestrator` or used a service unit named `openspec-orchestrator.service`, remove those before installing the rename:

```bash
sudo systemctl stop openspec-orchestrator 2>/dev/null
sudo systemctl disable openspec-orchestrator 2>/dev/null
sudo rm -f /etc/systemd/system/openspec-orchestrator.service /usr/local/bin/openspec-orchestrator
sudo systemctl daemon-reload
```

---
