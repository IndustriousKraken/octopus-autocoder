# code-reviewer — delta for a004-security-findings-block

## ADDED Requirements

### Requirement: Security-critical findings yield a Block verdict
A security-critical finding SHALL produce a `Block` verdict — never `Concerns` or `Pass`. Security-critical means: credential or secret leakage (a key, token, or secret written where it could be committed or otherwise exposed), hardcoded secrets, AND injection vulnerabilities. A credential leak is stop-the-line; surfacing it as a soft verdict that neither drafts the PR nor gates a merge is a mis-classification.

This is enforced in two layers:
- **Prompt rule.** The reviewer prompt SHALL instruct that credential/secret leakage, secret exposure, AND injection are `Block`-class findings, not `Concerns`.
- **Code-level safety net.** When the reviewer's own structured output flags a finding as a secret/credential/key exposure (or injection) AND returns a non-`Block` verdict, the daemon SHALL escalate the effective verdict to `Block`, so a mis-classifying model cannot downgrade a security-critical finding to advisory.

Non-security findings are unaffected — their verdicts are whatever the reviewer assigns.

#### Scenario: A credential-leak finding blocks
- **WHEN** the reviewer reports a finding that a key/secret/token is written where it could be committed or exposed
- **THEN** the effective verdict is `Block`
- **AND** the PR is drafted per the existing `Block` handling

#### Scenario: A non-Block verdict on a security finding is escalated
- **WHEN** the reviewer's output flags a secret/credential/key exposure (or an injection vulnerability) but returns `Concerns` or `Pass`
- **THEN** the daemon escalates the effective verdict to `Block`
- **AND** the escalation does not depend on the exact prose of the finding (it keys on the reviewer's own security-finding signal, not message wording)

#### Scenario: Non-security findings keep their verdict
- **WHEN** the reviewer reports only non-security findings (style, idioms, error handling, naming) with a `Concerns` verdict
- **THEN** the effective verdict stays `Concerns`
- **AND** no security escalation occurs
