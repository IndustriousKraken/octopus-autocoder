## ADDED Requirements

### Requirement: Polling-iteration triage flows resolve their prompts via the uniform PromptLoader
The polling iteration's two triage flows — the `send it` audit-reply triage AND the `propose` chat-request triage — SHALL load their prompt templates through `PromptLoader::load(PromptId::AuditTriage, &workspace_config)` AND `PromptLoader::load(PromptId::ChatRequestTriage, &workspace_config)` respectively. Direct `include_str!("../../prompts/audit-triage.md")` AND `include_str!("../../prompts/chat-request-triage.md")` invocations at the call sites SHALL be removed.

The override fields `executor.audit_triage.prompt_path` AND `executor.chat_request_triage.prompt_path` (per the executor spec) SHALL take effect for these flows. The loader's uniform precedence (embedded → per-workspace → daemon-level → embedded fallback) applies as documented.

#### Scenario: Send-it triage uses the loader
- **WHEN** the polling iteration processes a pending `send it` triage AND the workspace has no override configured
- **THEN** the executor invocation's prompt is the embedded `prompts/audit-triage.md` returned by the loader

#### Scenario: Send-it triage honors the per-workspace override
- **WHEN** the polling iteration processes a pending `send it` triage AND the workspace has `executor.audit_triage.prompt_path: "./prompts/triage-custom.md"` AND the file exists
- **THEN** the executor invocation's prompt is the override file's contents
- **AND** the LLM's classification behavior is governed by the operator's customized template

#### Scenario: Propose-flow triage uses the loader
- **WHEN** the polling iteration processes a pending `propose` request AND the workspace has no override configured
- **THEN** the executor invocation's prompt is the embedded `prompts/chat-request-triage.md` returned by the loader

#### Scenario: Propose-flow triage honors the per-workspace override
- **WHEN** the polling iteration processes a pending `propose` request AND the workspace has `executor.chat_request_triage.prompt_path: "./prompts/chat-triage-custom.md"` AND the file exists
- **THEN** the executor invocation's prompt is the override file's contents

#### Scenario: Missing override path falls back to embedded via the loader
- **WHEN** the workspace's triage override path is configured to a non-existent file
- **THEN** the loader's one-shot WARN fires per the executor spec's uniform precedence
- **AND** the triage flow proceeds with the embedded default
- **AND** the triage completes successfully (the misconfigured path is not a triage-blocking condition)
