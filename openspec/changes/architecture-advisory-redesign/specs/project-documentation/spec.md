## REMOVED Requirements

### Requirement: OPERATIONS.md describes the `.brightline-ignore` file and CHATOPS.md cross-links from `send it`

**Reason:** The `.brightline-ignore` file is removed with the duplicate-signature
metric that was its only consumer, so there is nothing for OPERATIONS.md to
document or for CHATOPS.md's `send it` section to cross-link. The OPERATIONS.md
and CHATOPS.md architecture-audit sections are rewritten for `architecture_advisor`
(advisory, recommendation-based, issue-by-default) as part of this change's docs
tasks; that replacement is general documentation, not a `.brightline-ignore`
subsection, so no successor requirement is added here.
