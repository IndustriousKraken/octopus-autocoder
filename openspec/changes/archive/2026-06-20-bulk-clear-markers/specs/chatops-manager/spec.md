## MODIFIED Requirements

### Requirement: Argument sanitization at parser entry
The parser SHALL sanitize every operator-supplied argument before passing it to file-path construction or control-socket dispatch. As a pre-sanitization hygiene step, the parser SHALL strip a single pair of surrounding backticks from each token returned by whitespace splitting (`token.trim_matches('\`')`) BEFORE applying the regex check; this accommodates the alert templates that wrap change slugs AND repo identifiers in single backticks for chat readability AND that operators routinely copy verbatim. Embedded (mid-token) backticks SHALL NOT be stripped; they continue to fail the regex check. Change-slug arguments SHALL match `^[a-zA-Z0-9_-]{1,64}$`; repo-substring arguments SHALL match `^[a-zA-Z0-9._/-]{1,128}$`. Malformed arguments (including arguments whose inner content fails the regex AFTER backtick stripping) SHALL produce `Some(Reply::Sync("✗ invalid <field>: ..."))` and SHALL NOT result in any file-system or control-socket call.

EXCEPTION — the wildcard sentinel: for the marker-clear verbs (`clear-perma-stuck` AND `clear-revision`) ONLY, a bare `*` token in the change-slug position OR the repo-substring position is a recognized WILDCARD SENTINEL — it is NOT a change-slug NOR a repo-substring. The parser SHALL recognize `*` for these two verbs BEFORE the regex check AND SHALL NOT reject it as malformed; the bulk-clear behavior it selects is governed by the orchestrator-cli "Marker-clear operator commands accept wildcard targets" requirement. This exception is narrow: it applies to `*` for those two verbs only. Every other token — every non-`*` argument, AND every argument of every other verb — is sanitized exactly as above (a non-matching argument is still rejected with `✗ invalid <field>` AND still reaches neither the filesystem nor the control socket).

#### Scenario: Path-traversal in change name is rejected
- **WHEN** `handle_message("<@UBOT> clear-perma-stuck myrepo ../../etc/passwd", ...)` is called
- **THEN** the return value is `Some(Reply::Sync(text))` where `text` begins with `✗ invalid change name`
- **AND** no control-socket submission is performed
- **AND** no `std::fs::*` call is made

#### Scenario: Shell metacharacter in change name is rejected
- **WHEN** `handle_message("<@UBOT> clear-perma-stuck myrepo a; rm -rf /", ...)` is called
- **THEN** the return value is `Some(Reply::Sync(text))` where `text` begins with `✗ invalid change name`
- **AND** no control-socket submission is performed

#### Scenario: Oversized argument is rejected
- **WHEN** a change name with more than 64 characters is supplied
- **THEN** the return value is `Some(Reply::Sync(text))` where `text` begins with `✗ invalid change name`

#### Scenario: Valid arguments pass through
- **WHEN** valid arguments such as change name `a06-foo` and repo substring `your-org/your-repo` are supplied
- **THEN** the parser returns the recognized `OperatorCommand` variant
- **AND** the dispatcher proceeds normally

#### Scenario: Surrounding backticks on a change slug are stripped before regex check
- **WHEN** `handle_message("<@UBOT> clear-revision myrepo \`a37-unify-llm-provider-config\`", ...)` is called
- **THEN** the parser strips the surrounding backticks AND the regex check sees `a37-unify-llm-provider-config`
- **AND** the parser returns `Ok(OperatorCommand::ClearRevision { repo_substring: "myrepo", change: "a37-unify-llm-provider-config" })`
- **AND** no `✗ invalid change name` reply is produced

#### Scenario: Surrounding backticks on a repo substring are stripped before regex check
- **WHEN** `handle_message("<@UBOT> clear-revision \`myrepo\` a37-foo", ...)` is called
- **THEN** the parser strips the surrounding backticks AND the regex check sees `myrepo`
- **AND** the parser returns `Ok(OperatorCommand::ClearRevision { repo_substring: "myrepo", change: "a37-foo" })`
- **AND** no `✗ invalid repo substring` reply is produced

#### Scenario: Embedded backticks remain rejected
- **WHEN** `handle_message("<@UBOT> clear-revision myrepo a37\`foo", ...)` is called
- **THEN** the strip step is a no-op (the backtick is mid-token, NOT surrounding)
- **AND** the regex check rejects `a37\`foo`
- **AND** the return value is `Some(Reply::Sync(text))` where `text` begins with `✗ invalid change name`

#### Scenario: Backtick-wrapped shell-metacharacter payload remains rejected
- **WHEN** `handle_message("<@UBOT> clear-perma-stuck myrepo \`a;rm -rf /\`", ...)` is called
- **THEN** the strip step yields `a;rm -rf /`
- **AND** the regex check rejects it
- **AND** the return value is `Some(Reply::Sync(text))` where `text` begins with `✗ invalid change name`
- **AND** no control-socket submission is performed
- **AND** no `std::fs::*` call is made

#### Scenario: Asymmetric backticks are stripped
- **WHEN** `handle_message("<@UBOT> clear-revision myrepo \`a37-foo", ...)` is called (leading backtick only, no trailing backtick)
- **THEN** the strip step removes the leading backtick AND the regex check sees `a37-foo`
- **AND** the parser returns `Ok(OperatorCommand::ClearRevision { repo_substring: "myrepo", change: "a37-foo" })`
- **AND** the same shape applies symmetrically when only the trailing backtick is present (`a37-foo\``)

#### Scenario: Wildcard sentinel is accepted for the marker-clear verbs
- **WHEN** `handle_message("<@UBOT> clear-revision myrepo *", ...)` OR `handle_message("<@UBOT> clear-revision *", ...)` is called
- **THEN** the `*` is recognized as the wildcard sentinel — it is NOT rejected as `✗ invalid change name` / `✗ invalid repo substring`
- **AND** the parser returns the marker-clear command in its wildcard form
- **AND** a non-`*` token in the same position is still regex-checked (a malformed slug or repo substring is still rejected)
