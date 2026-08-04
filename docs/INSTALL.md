# Manual install from source

For contributors and operators who specifically want to avoid downloaded binaries. The steps below take a checkout from `git clone` to a running daemon in about ten minutes. Each step is self-contained; do them in order.

## 1. Prerequisites

On the machine where the daemon will run:

- **Rust toolchain.** Install via [rustup](https://rustup.rs/) — autocoder builds against stable Rust on edition 2024.
- **Claude Code authenticated.** Install [Claude Code](https://www.anthropic.com/claude-code) and run `claude auth login` as the same OS user that will run the daemon. The credentials are persisted in `~/.claude/` and survive restarts.
- **OpenSpec CLI installed and on `$PATH`.** Install with `npm install -g @fission-ai/openspec` (Node.js required) and verify with `openspec --version`. autocoder shells out to `openspec instructions apply` to build richer per-change prompts for the agent; without it the executor falls back to raw markdown concatenation, which gives the agent noticeably less guidance and is a known cause of "lazy archive" failures.
- **A GitHub Personal Access Token**, scoped to the repositories autocoder will manage. Either form works; pick based on your account setup:

  - **Fine-grained PAT** (recommended for personal-account-owned repos). Required permissions:
    - **`Pull requests: read & write`** — needed for PR creation.
    - **`Administration: write`** — needed ONLY if you use `github.fork_owner` (fork-PR mode) AND not all forks already exist; autocoder calls `POST /forks` for missing ones.
    - **`Contents: read & write`** — needed ONLY if your `config.yaml` uses HTTPS URLs (`https://github.com/...`); when you use SSH URLs (`git@github.com:...`), git authenticates via your SSH key and `Contents` is not required.
    - **`Issues: read & write`** — needed ONLY in the rare case that your host rejects draft PRs and triggers the `do-not-merge` label fallback. GitHub.com supports drafts on every repo type, so this is not needed there; only relevant for some private GHE configurations.

    Fine-grained PATs are scoped to a single account or organization; multi-owner setups use [Multiple GitHub Tokens](CONFIG.md#multiple-github-tokens) instead.

  - **Classic PAT** (simpler when the minting account itself has scoped repo access — e.g. a machine user added as Read collaborator on specific repos). Required scope: **`repo`** (covers PR creation, label apply, and HTTPS git ops). The PAT's effective access intersects with the minting user's actual repo permissions, so a classic PAT minted by a scoped-access machine user is effectively scoped to those repos. Tradeoff: future repo additions (new team membership, new collaborator invite) automatically extend the PAT's reach; fine-grained requires re-minting. Also: some orgs require fine-grained PATs at the org-policy level (Settings → Personal access tokens → "Restrict access via fine-grained personal access tokens"); check before committing to classic.

  Export the token as `GITHUB_TOKEN` in the environment that will launch the daemon, or use the inline form in `config.yaml` (see [Secrets in `config.yaml`](SECURITY.md#5-secrets-in-configyaml-inline-vs-env-var)).
- **`git` configured.** Either a registered SSH key for the configured repository URLs (recommended), or HTTPS credentials in a credential helper.
- **A usable platform sandbox mechanism.** Every agentic subprocess is kernel-wrapped before it runs. On Linux this is **bubblewrap** (`bwrap`, e.g. `apt-get install bubblewrap`) — which needs *unprivileged user namespaces enabled* — or, when the daemon runs under systemd, transient `systemd-run` service mode. On macOS it is **`sandbox-exec`** (ships with the OS). Without a usable mechanism the daemon fails its startup preflight (and agentic runs fail closed unless you set `executor.sandbox.allow_unsandboxed`). Run `autocoder doctor` (below) to confirm the mechanism is not just present but *usable* on your host.

## 2. Clone and configure

```bash
git clone https://github.com/IndustriousKraken/octopus-autocoder.git
cd octopus-autocoder
cp config.example.yaml config.yaml
```

Edit `config.yaml` and set the `url:` value to your repository. The shipped example uses `git@github.com:your-org/your-repo.git` as a placeholder.

## 3. Build the daemon

```bash
cd autocoder
cargo build --release
sudo cp target/release/autocoder /usr/local/bin/autocoder
cd ..
mkdir -p ~/autocoder
cp config.yaml ~/autocoder/config.yaml
chmod 600 ~/autocoder/config.yaml
```

The build produces a `~10 MB` self-contained binary. The implementer prompt template (`prompts/implementer.md`) and the code-reviewer template (`prompts/code-review-default.md`) are both embedded at compile time, so the runtime needs only `config.yaml`. To override either template, set `executor.implementer_prompt_path` or `reviewer.prompt_template_path` in `config.yaml` to a path on disk. The `--config` flag accepts any absolute path.

## 4. Run it

```bash
export GITHUB_TOKEN=ghp_yourfinegrained_token_here
RUST_LOG=info autocoder run --config ~/autocoder/config.yaml
```

> **Multiple GitHub accounts/orgs?** Skip the `GITHUB_TOKEN` export and use the [Multiple GitHub Tokens](CONFIG.md#multiple-github-tokens) section to configure `github.owner_tokens:` in `config.yaml` instead.

You should see (within a few seconds):

```
INFO autocoder: configured repository url=... workspace=/tmp/workspaces/... poll_interval_sec=300
INFO autocoder: starting polling loop ...
INFO autocoder: polling pass produced no changes
```

If your repository's `openspec/changes/` directory contains a ready change, the daemon picks it up on the next iteration, runs the Claude CLI against it, commits the diff, pushes the agent branch, and opens a PR.

To stop the daemon: `Ctrl-C` (SIGINT). It drains the current iteration and exits within ~30 seconds.

## 5. (Optional) Verify against a sandbox

[`docs/foundation-smoke-test.md`](docs/foundation-smoke-test.md) walks through scaffolding two throwaway GitHub repos with trivial OpenSpec changes and confirming the full clone → execute → commit → push → PR cycle works against them. Recommended for first-time deploys.

## 6. Check dependencies with `autocoder doctor`

Before (or any time after) starting the daemon, run the dependency preflight on demand:

```bash
autocoder doctor                       # uses the systemd-unit / default config if present
autocoder doctor --config ~/autocoder/config.yaml
```

`doctor` runs the **same comprehensive preflight the daemon runs at startup** and prints one report covering every dependency at once — it never stops at the first failure:

- **Required** — `openspec`, `git`, and a *usable* platform sandbox mechanism. A missing or unusable required dependency makes `doctor` exit non-zero (and aborts daemon startup) with a message naming it and how to install it. The sandbox check verifies the mechanism actually **applies** the sandbox — e.g. `bwrap` present but with unprivileged user namespaces disabled is reported **unusable**, not satisfied.
- **Configuration-implied** — the agent-CLI binary for each configured strategy (e.g. `claude`/`opencode`), the `gh` CLI when the scout feature is on, and the embedding backend when canonical-specs RAG is on. These are reported and warned when missing, but do not abort startup.

The same checks run automatically at `autocoder run` startup, so a misconfigured host fails loudly instead of looping. The assisted installer (`install.sh` → `autocoder install`, see [DEPLOYMENT.md](DEPLOYMENT.md)) offers to install the OS-package dependencies for you with per-step consent.

## 7. End-to-end testing (optional) {#7-end-to-end-testing-optional}

Needed only for repositories configured with an [`app_under_test`](CONFIG.md) block, where autocoder verifies a change by running the application.

The toolchain splits in two, and only one half needs root:

- **System packages** — the shared libraries the browser links against. Requires elevated privileges.
- **Browser binaries** — a user-space download, placed in a daemon-owned directory under the resolved cache dir (not the service account's `~/.cache`), so the daemon, the executor session, and the end-to-end command all resolve the same location.

The installer provisions both, with a consent step each:

```bash
sudo autocoder install --reconfigure e2e
```

`--reconfigure` works against an existing installation, so adding end-to-end verification never requires a reinstall or hand-run package commands. The section is idempotent, and on a host with no supported package manager it reports what it could not provision plus the manual commands, without aborting the rest of the installation.

**Provisioning is operator-initiated only** — never during a pass, never at daemon startup. autocoder will not download a browser runtime as a side effect of polling. When the runtime is missing, `doctor` and the startup preflight warn per declaring repository, naming the command above, and end-to-end verification stays disabled for that repository only. Other repositories are unaffected and the daemon starts normally.

Provisioning delegates to the browser tool's own dependency resolver rather than a package list carried in this repository, because package names are distro-**version**-specific (Ubuntu 24.04's 64-bit `time_t` transition renames `libatk1.0-0` to `libatk1.0-0t64`, for instance). It installs a broad set — roughly 45 packages on Ubuntu 24.04, about a third of them fonts, which materially improve rendering fidelity for screenshot-based debugging. On a constrained host the strictly-required subset is ten packages:

```bash
apt-get install -y libnss3 libnspr4 libatk1.0-0t64 libatk-bridge2.0-0t64 \
  libatspi2.0-0t64 libgbm1 libxcomposite1 libxdamage1 libxfixes3 libxrandr2
```

A headless server needs no display server, GPU, or desktop environment: the browser runs headless, and Playwright's headless shell already passes `--no-sandbox` and `--disable-dev-shm-usage` itself, so no extra flags are required inside autocoder's OS sandbox.

---
