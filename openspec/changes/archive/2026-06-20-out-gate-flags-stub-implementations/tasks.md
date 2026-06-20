# Tasks

## 1. Strengthen the embedded [out] prompt

- [x] 1.1 In `prompts/code-implements-spec-check.md`, add an explicit instruction that a requirement the spec expects to be backed by working code is NOT satisfied by a stub or a deferral, naming the forms (placeholder/hardcoded/faked return, `todo!()`/`unimplemented!()`/`panic!("not implemented")`, an unconditional early-return that skips the required path, an unwired branch or error path, a config flag read but never acted on, a "wire this up in a follow-up" deferral). Instruct the verifier to classify a wholly-stubbed requirement as `missing` and a half-wired one as `partial`, with the stub as the evidence, AND to flag it whether or not the delta separately says "do not stub."

## 2. Tests

- [x] 2.1 Keep the existing embedded-prompt sanity test green (the prompt remains non-empty AND still references `submit_verdict`); do NOT assert the new wording verbatim (prompt content is data, not behavior). If a tripwire is wanted, assert only on a stable derived property, not the prose.
