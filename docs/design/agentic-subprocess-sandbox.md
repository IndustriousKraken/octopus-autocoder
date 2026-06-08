# Agentic-subprocess sandbox — credential & filesystem isolation — design

**Status:** converged design, not yet specced. Captures the security architecture for isolating every model-running subprocess from credentials and the rest of the host. Companion to the `a003` key-flow change (which ensures keys are never *passed* to a subprocess); this note is the "the model can't go *get* them either" enforcement.

## Why

The a60/PR-#95 incident — `OpencodeStrategy` wrote a plaintext provider key into a workspace `opencode.json` — exposed the deeper rule: **no model, in any role, should be able to reach a credential or other sensitive host data.** And it must be enforced from *outside* the CLI. Each CLI's own "I won't let the model look around" sandbox is a promise from the thing we're trying to contain — trusting it is anti-security. The kernel enforces the boundary; the CLI does not get a vote.

## Scope — every agentic subprocess, by construction

Every agentic role funnels through the **single** subprocess spawn in `agentic_run`: the executor, the agentic reviewer, every audit (drift, security-bug, architecture, missing-tests, docs, brightline-triage), the contradiction check, the changelog-stylist, triage, scout, brownfield. The sandbox is a property of that one seam — wrap the spawn (`<sandbox> … -- <cli> …`) and no role can opt out.

Per-role variation is only:
- **tool allowlist** — read-only roles (reviewer, audits) get Read/Glob/Grep and no Bash/Write; the executor gets the write set;
- **workspace mode** — executor mounts the workspace rw; reviewer/audits mount it ro (strictly less surface).

Everything else — the filesystem whitelist, capability drops, `/proc` restriction — is **identical for every role**.

The one role with no model-subprocess is the non-agentic `oneshot` HTTP reviewer: autocoder makes that API call in-process, the key stays in autocoder's memory, and no model is involved. Same principle, opposite side.

## The credential principle

The key is a property of the **CLI process**, never of the model:
- The CLI process authenticates by injecting its key into the TLS header in its own memory (the way OpenCode does), in isolation from the model.
- The **model** is the entity tunneled across that authenticated connection. It never needs the key.

So for CLI roles, **autocoder holds no LLM keys at all** — the CLI self-authenticates from its own store (`~/.claude`, opencode's config dir / Big-Pickle, `~/.antigravity`). Keys live in autocoder only for the in-process HTTP roles, and never reach a subprocess (file or env — env is readable from Bash, and for Anthropic an env key also forces pay-per-token off the subscription).

**Why the model must never even *see* a key.** Theft (read-then-POST) is only half of it. Anything in the model's context is transmitted to the provider and, conservatively, **disclosed to them permanently** — you can't verify a provider's retention or training pipeline, the TOS can change, and the data is physically theirs regardless. A key that merely enters context is therefore compromised at the moment of the *read*, before any exfil attempt — and a *cross-provider* key so disclosed (your OpenAI key landing in Anthropic's context) is irreversibly burned. Prevention — keep the key out of context — is the only control that addresses this; egress-locking and detection do not (see the threat model's passive-exfil row).

## Threat model

| Vector | Control |
|---|---|
| Read a credential file (`~/.claude/.credentials.json`, autocoder config, `~/.ssh`) | Filesystem **whitelist** — those paths are not in the sandbox namespace. (Bash denylists can't do this — `cat`/`head`/`python -c open()` is whack-a-mole; the kernel/namespace can.) |
| Read the CLI process's memory or env | Drop `CAP_SYS_PTRACE` + tighten `ptrace_scope`; mount `/proc` with `hidepid` (no `/proc/<cli>/mem` or `…/environ`); and no key is in env to begin with |
| Sniff traffic (tcpdump/wireshark, incl. the workspace's own proxy project) | Drop `CAP_NET_RAW` (no raw sockets) / `RestrictAddressFamilies` without `AF_PACKET` |
| Reroute or MITM the CLI's connection | Drop `CAP_NET_ADMIN` (no iptables/route hijack); `/etc` ro/absent (no `hosts` edit); no CA-store write (no TLS MITM); the CLI's `HTTPS_PROXY` is autocoder-set and can't be changed by a child |
| **Run a proxy** — the workspace's own, or a Python one the model writes | **Inert.** A proxy only sees traffic routed *through* it, and the model can't route the CLI's established connection through anything (the controls above). So the model may compile/run any proxy — there's no CLI credential flowing through it, nothing to read. We do **not** block proxies — that would break the very testing the executor exists to do. |
| Exfiltrate a key it somehow obtained | **Out of network scope.** Egress allowlisting is deliberately not implemented — there is no maintainable global allowlist for CDN'd API/forge hosts, and IP-pinning is too fragile (see open decisions). The control is *upstream*: the model never obtains a key (FS-whitelist + OS-hide + engine-deny). If credential protection is disabled, or a key is supplied via env, outbound exfil is possible and is the operator's accepted risk. |
| **Passive exfil — a key in context is already on the provider's servers** | Egress allowlist **cannot** cover this: the provider endpoint is necessarily allowed, so a key read into the model's context ships to the provider as ordinary traffic. The leak is the *read*, not a later POST — detection is moot, only prevention works. Worst for **cross-provider** keys (your OpenAI key in Anthropic's context = disclosed to a different provider, irreversibly). This is why other-CLI configs default to the fail-closed boundary, not the deter layer. |
| **Spawn a child CLI and MITM it** — model sets `HTTPS_PROXY` + a CA-override (`NODE_EXTRA_CA_CERTS`/`SSL_CERT_FILE`) for a `claude`/`agy` it launches, routing the child's auth through a proxy it runs | **No escalation.** A child of a *different* CLI can't authenticate (its auth store isn't mounted) — nothing to intercept. A child of the *same* CLI can be MITM'd, but yields only the same token the model could already `cat` from the one mounted store (the same-uid residual) — bounded to one credential (not network-contained; exfil is out of scope per the operator's accepted risk). Closure: a **credential broker** (no flat-file token to read or MITM) would fully close it, but is deferred (see open decisions). Note that "keep the CLI binaries out of the model's exec namespace" is **not** a real closure here — the threat is the direct read of the one mounted auth file, which a nested CLI only reaches more elaborately; removing the binary protects nothing the direct read doesn't already expose. |
| Exfil via workspace → commit → push | Requires the model to *have* a key to write into a file, which the read-controls deny. Belt-and-braces: push is daemon-only, plus **forge-side push protection** (GitHub/GitLab secret scanning); an in-process pre-push scan is deferred (see open decisions). |

**The honest residual:** the CLI's auth file and the model's Bash run under the **same uid**, so the OS sandbox alone can't let the CLI read its auth while forbidding the model's Bash from reading the same file. A few things contain it:
1. each run mounts **only that one CLI's** auth store → worst case a model sees *its own* session token, never another provider's (no cross-key sprawl — "Qwen with Anthropic keys" can't happen);
2. that self-token is the CLI's *own* provider credential — the same one it uses legitimately — so disclosure means "impersonate this CLI to its own provider," not reach to other providers or other secrets;
3. where a CLI supports a **credential helper / keychain** (secret is not a flat readable file), use it and the same-uid read disappears;
4. the CLI's own **engine-deny** rules block the model's Read/Bash tools from that store — a string-match speed bump (`xxd`/`python -c open()`/bracket-globs evade it), so a *deter*, not a boundary (see the config-credential policy below).

With egress allowlisting out of scope, this self-token is **not** network-contained — if it reaches context it can leave. The real defense is therefore *prevention* (keep it out of context) plus the limited blast radius above.

So the only trust left is narrow and explicit: a CLI does not *deliberately* paste its own key into its own model's context — and if one is that sloppy, the blast radius is its single credential, egress-locked, not the keyring.

A **credential broker** (key held by a separate-uid helper, never in the CLI's namespace) would erase this residual entirely, but is **deferred** — operators run these CLIs un-brokered today, and the bounded, egress-locked self-token residual is an acceptable cost for now.

## Config-credential protection policy — two layers, two toggles

The threat model above hides credential *files* and locks egress. The CLI-config stores (`~/.claude`, opencode's dir, `~/.antigravity`) are a special sub-case for two reasons: the **self**-CLI must read its own store to authenticate, and a repo whose job is *wrapping* CLIs may need a **nested** CLI to authenticate live. Two independent layers cover them, and they fail in opposite directions:

- **OS-hide — allowlist, fail-closed (depend on this).** The filesystem whitelist simply doesn't admit a CLI's config dir; a cred path nobody thought to enumerate is hidden anyway (default-deny). It **cannot** cover the *self*-config — the running role's own CLI must read it to auth — so OS-hide applies to **other** CLIs' configs only.
- **Engine-deny — denylist, fail-open (deter with this).** Deny rules in each CLI's own permission engine block the **agent's** Read/Bash tools from the config paths, while the CLI's **native** auth read (not a tool call) still succeeds. This covers what OS-hide can't (the self-config), but it is a string-match speed bump — `xxd`, `python -c open()`, bracket-globs, base64'd paths evade it. Deter, never depend.

Running **both** by default is deliberate: the boundary (OS-hide) holds even when the deny list goes stale, and the deny list still deters the one thing the boundary can't reach (self-reading-self).

The primitive is **two orthogonal booleans — `os_hide` and `engine_deny`** — each settable per-repo and globally. The named levels are presets over those two switches:

| Preset | `os_hide` | `engine_deny` | When |
|---|---|---|---|
| **Default** | on | on | Normal repos. Other configs hidden (boundary); self-config deterred (speed bump). |
| **Per-repo L1** | off | on | This repo wraps CLIs and needs a nested CLI to authenticate *live*. Other configs reachable, but cross-CLI reads still deterred. |
| **Per-repo L2** | off | off | This repo's job is testing credential-grab / threat-intel against the CLIs; everything open. |
| **Global L1** | off | on | Operator develops only CLI-wrapping apps. |
| **Global L2** | off | off | Operator's repos all have special requirements (cred-grab testing, foreign-model watch, API-kludge testing). Least common, still plausible. |
| *(4th cell)* | on | off | Reachable because `engine_deny` is independently disablable: boundary on, no self-deter — fine if you accept the self-read (the provider already holds its own token). |

**Scope — these toggles govern the config-credential layer only.** The rest of the sandbox — workspace FS whitelist, capability drops, `/proc` restriction — stays **on in every preset, including Global L2**. `os_hide=off` never means "no sandbox"; it means "other CLIs' config dirs are admitted to the otherwise-still-whitelisted namespace."

**Implementation:**
- Engine-deny rules are supplied **per-invocation** through each CLI's own settings mechanism (a run-scoped config autocoder controls) — **never** by mutating the operator's global `~/.claude/settings.json` (or the opencode/agy equivalents). Editing the global config would litter and clobber the operator's own rules and persist past the run. Disabling is then trivial: don't supply them.
- Each CLI's deny list is the **union of all known CLI config paths** (self + others), so that under L1 (OS-hide off) cross-CLI reads are deterred, not just self-reads.
- The deny list is a **maintained, versioned artifact** in autocoder, updated as CLIs move their cred stores. Because it fails open, a stale entry silently exposes a path — which is exactly why it is the deter layer and OS-hide (fail-closed) is the depend layer.

**Wrapper-dev carve-out (why L1/L2 are rarely needed).** The agent's real job on a wrapper is the **structural probe** — `--version`, flag acceptance, config/MCP-file parsing, output-format handling — none of which needs the wrapped CLI authenticated (it starts and answers these without its cred store). The **live** model round-trip through the wrapped CLI is dev/staging organic validation, where the operator controls exposure. So even a wrapper repo usually runs the Default; L1 is only for letting the *agent itself* perform a *live* cross-CLI round-trip — a narrow, explicit opt-in.

**Precedence (proposed).** Per-repo overrides global; absent a per-repo setting, global applies; absent both, the secure Default (on/on). Loosening is always explicit — no implicit downgrade — and any run with either toggle off **logs it loudly at startup** (e.g. `os_hide=off for <repo>: other-CLI configs reachable`), so a relaxed posture is never silent.

**The action stream is not an enforcement seam.** Autocoder's stream of model actions is downstream telemetry, not a control plane — it reports built-in Bash/Read calls *after* dispatch, with no veto channel back into the CLI. Enforcement lives at the OS (allowlist) and the CLI engine (deny rules); the kernel's denial record is the audit trail. The stream is observe-only and must not be re-proposed as a credential-access interceptor.

**Deployment note — this repo.** autocoder itself wraps CLIs, so under the secure Default its own live cross-CLI development breaks (other-CLI configs are hidden). When the sandbox ships, set the **Per-repo L1** override for this repo (`os_hide=off`, `engine_deny=on`), or run its live cross-CLI tests outside the sandbox. Structural probing (`--version`, flag/parse checks) is unaffected and works under the Default.

## Mechanisms (Linux-native, external to the CLI)

| Option | Covers | Notes |
|---|---|---|
| **`systemd-run` (transient service)** with sandbox properties | FS (`ProtectSystem=strict`+`ReadWritePaths=`, `ProtectHome=tmpfs`+`BindReadOnlyPaths=`, `InaccessiblePaths=`), net (`RestrictAddressFamilies` — egress allowlist out of scope), caps (`CapabilityBoundingSet`), `ProtectProc`/`ProcSubset`, `NoNewPrivileges` | **Chosen primary** — systemd deployment; covers all three threat classes declaratively per-invocation, no extra binary. Must run in **service mode, not `--scope`**: the `Protect*`/`Bind*` FS directives apply only when PID 1 execs the unit (a scope gets cgroup props like `IPAddressAllow` but not the mount sandbox). Capture stdout with `--pipe --wait --collect`. |
| **bubblewrap** (`bwrap`) | Mount-namespace whitelist (bind only workspace + binary/libs + one auth store) | Finer mount control; mature (Flatpak); unprivileged |
| **Landlock + seccomp** | Kernel-enforced FS allowlist + syscall filter, applied pre-`exec`, inherited and inescapable | Elegant, no namespace setup; FS-focused (pair with a netfilter for egress) |
| **nsjail / container** | Full namespace jail | Heaviest. (`nsjail` is what Antigravity's own sandbox uses — i.e. doing externally what it asks us to trust it to do internally.) |

## Open decisions

- **Primary mechanism:** **decided — `systemd-run` (transient service mode)**, with `bwrap` the fallback for unprivileged / non-systemd / in-container hosts (no root PID 1). `Landlock+seccomp` not pursued as primary (FS-only; would still need a netfilter for egress).
- **Egress allowlisting — decided: out of scope.** No maintainable global allowlist exists for CDN'd API/forge hosts and IP-pinning is too fragile; there is no current demand. More fundamentally, outbound filtering belongs at the host/network **firewall** layer — not in an application daemon; this project builds credential and filesystem isolation, not a network appliance. Outbound traffic is therefore unrestricted at our layer; credentials are defended by *prevention* (the read-controls), not network containment. An operator who wants egress control configures their firewall; a configurable in-app allowlist is at most a far-future convenience.
- **Pre-push secret scan — decided: rely on forge push protection** (GitHub/GitLab secret scanning) rather than a local scan. A local pass would be either a `gitleaks` shell-out (~20–40 LoC, but adds a binary to install/version + false-positive tuning that could block an autonomous push) or a hand-rolled in-process regex (~100–150 LoC, no deps, narrower coverage). The read-controls already deny the model a key to commit, so neither is worth the surface now; an optional in-process regex scan remains a possible future feature.
- **Credential broker** — *deferred.* Removes the same-uid self-token residual entirely (no flat-file token to read), but adds a separate-uid proxy/helper surface. Un-brokered is the accepted posture for now; revisit if the self-token residual proves to matter.
- **Toggle precedence — decided:** per-repo overrides global; default-secure (on/on); loosening always explicit and logged loudly at startup.
- **`DynamicUser` (ephemeral uid) — decided: not pursued.** Adds workspace-ownership friction for little gain: the FS whitelist already keeps autocoder's own config out of the sandbox, and `DynamicUser` would not close the same-uid self-token residual anyway (the self-CLI store must be readable by whichever uid runs the CLI — the same uid as its agent's Bash). Revisit only on a concrete need.

## Sequencing

- Depends on `a56` (the `agentic_run` seam). Independent of but complementary to **a003** (key-flow: keys are never *passed* in): a003 ensures nothing hands a key to a subprocess; this note ensures a subprocess can't reach one anyway.
- This is the load-bearing "*actually* sandboxed" piece — the others (a004 security→Block, a005 aggregate revisions) don't depend on it.
