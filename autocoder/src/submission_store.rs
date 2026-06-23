//! Daemon-side execution-scoped submission store (a56).
//!
//! Parallels [`crate::outcome_store`]: holds the schema-validated payloads
//! that the per-execution MCP child relays via the `record_submission`
//! control-socket action, keyed by `(workspace_basename, change)`. The
//! role's daemon-side caller drains the store via `consume_submission`
//! AFTER the wrapped CLI exits.
//!
//! This change establishes the transport AND lifecycle. The concrete
//! per-role `submit_*` tools AND their payload schemas are registered by
//! the changes that consume them (the reviewer, contradiction-check, etc.);
//! a role with no registered schema accepts any payload. The seam those
//! changes plug into is [`SubmissionStore::register_schema`].
//!
//! Lifecycle: in-memory only, like the outcome store. A daemon restart
//! loses any in-flight entries; submission is synchronous (the tool call
//! happens milliseconds before the wrapped CLI exits AND `consume` runs
//! microseconds after).

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// A per-role payload validator. Returns `Ok(())` when the payload
/// satisfies the role's schema, or `Err(reason)` with a correction-
/// suitable message the MCP relay surfaces to the agent.
pub type SubmissionValidator = Arc<dyn Fn(&Value) -> Result<(), String> + Send + Sync>;

/// Shared, mutex-protected store for in-flight submissions plus the
/// per-role schema registry. Cheap to clone: state lives behind `Arc`s.
#[derive(Clone, Default)]
pub struct SubmissionStore {
    inner: Arc<Mutex<HashMap<(String, String), Value>>>,
    schemas: Arc<Mutex<HashMap<String, SubmissionValidator>>>,
    /// Keys `(workspace_basename, change)` for which a schema-valid submission
    /// was EVER relayed (recorded) by a session's MCP child — retained even
    /// after the entry is consumed/drained (verifier-gates-persist-session-log
    /// task 4.2). Lets a `consume` that finds NO live entry distinguish
    /// "relayed but not consumed in time / already drained" (mode c, the relay
    /// reached the daemon) from "never relayed" (mode b, the model never called
    /// the submit tool). Observability only — never gates control flow.
    relayed: Arc<Mutex<HashSet<(String, String)>>>,
    /// Per-session record of which submission tool the MCP child advertised at
    /// `tools/list` time, keyed by `(workspace_basename, change)` and carrying
    /// `(role, Option<tool>)`: `Some(tool)` when a `submit_*` tool matched the
    /// session's role, `None` when none did (verifier-gates-persist-session-log
    /// task 4.1). Recorded daemon-side (not via CLI stderr, which opencode may
    /// not forward) so a held no-submission `consume` can report "tool never
    /// advertised" (mode a) regardless of the wrapped CLI. Retained across
    /// `consume` — it is a "what was advertised" fact, not the live payload.
    /// Observability only — never gates control flow.
    advertised: Arc<Mutex<HashMap<(String, String), (String, Option<String>)>>>,
}

impl SubmissionStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a per-role payload validator. The concrete `submit_*`
    /// tools added by later changes register their schema here; this
    /// change registers none, so every role accepts any payload until
    /// then. Tests register a validator to exercise the rejection path.
    #[allow(dead_code)]
    pub fn register_schema(&self, role: impl Into<String>, validator: SubmissionValidator) {
        self.schemas
            .lock()
            .expect("submission schema registry mutex poisoned")
            .insert(role.into(), validator);
    }

    /// Validate `payload` against `role`'s registered schema (Ok when no
    /// schema is registered), then store it keyed by `(workspace_basename,
    /// change)`. On a validation failure NOTHING is stored AND the reason
    /// is returned for the relay to surface to the agent. Last-writer-wins
    /// on the key, matching the outcome store's retry semantics.
    pub fn record(
        &self,
        workspace_basename: String,
        change: String,
        role: &str,
        payload: Value,
    ) -> Result<(), String> {
        if let Some(validator) = self
            .schemas
            .lock()
            .expect("submission schema registry mutex poisoned")
            .get(role)
            .cloned()
        {
            validator(&payload)?;
        }
        let key = (workspace_basename, change);
        // Mark that a submission was relayed for this key BEFORE storing, so a
        // later consume that finds no live entry can still report that the
        // relay reached the daemon (mode c) vs never (mode b). Retained across
        // consume — it is a "was ever relayed" fact, not the live payload.
        self.relayed
            .lock()
            .expect("submission relayed-set mutex poisoned")
            .insert(key.clone());
        self.inner
            .lock()
            .expect("submission store mutex poisoned")
            .insert(key, payload);
        Ok(())
    }

    /// Atomically read AND remove the entry for `(workspace_basename,
    /// change)`. Subsequent calls for the same key return `None`.
    pub fn consume(&self, workspace_basename: &str, change: &str) -> Option<Value> {
        self.inner
            .lock()
            .expect("submission store mutex poisoned")
            .remove(&(workspace_basename.to_string(), change.to_string()))
    }

    /// Whether a schema-valid submission was EVER relayed for
    /// `(workspace_basename, change)` — true even after the entry was consumed.
    /// Used by `consume_submission` to distinguish "relayed but not consumed"
    /// (mode c) from "never relayed" (mode b) for a held no-submission gate
    /// (verifier-gates-persist-session-log task 4.2).
    pub fn was_ever_relayed(&self, workspace_basename: &str, change: &str) -> bool {
        self.relayed
            .lock()
            .expect("submission relayed-set mutex poisoned")
            .contains(&(workspace_basename.to_string(), change.to_string()))
    }

    /// Record which submission tool the MCP child advertised for the session's
    /// role at `tools/list` time (verifier-gates-persist-session-log task 4.1).
    /// `tool` is `Some(name)` when a `submit_*` tool matched `role`, `None` when
    /// none did. Keyed by `(workspace_basename, change)` and retained across
    /// `consume`, so a no-submission `consume` can report whether the tool was
    /// ever advertised (mode a). Last-writer-wins on the key. Observability
    /// only — never gates control flow.
    pub fn record_advertised_tool(
        &self,
        workspace_basename: String,
        change: String,
        role: String,
        tool: Option<String>,
    ) {
        self.advertised
            .lock()
            .expect("submission advertised-map mutex poisoned")
            .insert((workspace_basename, change), (role, tool));
    }

    /// The submission tool the MCP child advertised for `(workspace_basename,
    /// change)`, as `(role, Option<tool>)` — `Some((role, Some(tool)))` when a
    /// `submit_*` tool was advertised, `Some((role, None))` when the role had no
    /// matching tool, and `None` when no advertisement was ever recorded for the
    /// key (e.g. the MCP child never reached `tools/list`, or the record was
    /// best-effort dropped). Survives `consume`. Used by `consume_submission` to
    /// report mode (a) — "tool never advertised" — alongside relayed/consumed.
    pub fn advertised_tool(
        &self,
        workspace_basename: &str,
        change: &str,
    ) -> Option<(String, Option<String>)> {
        self.advertised
            .lock()
            .expect("submission advertised-map mutex poisoned")
            .get(&(workspace_basename.to_string(), change.to_string()))
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn record_then_consume_returns_payload_and_clears() {
        let store = SubmissionStore::new();
        let payload = json!({"verdict": "approve", "notes": "looks good"});
        store
            .record("my-repo".into(), "a30-foo".into(), "reviewer", payload.clone())
            .expect("no schema registered → accepts");
        let got = store.consume("my-repo", "a30-foo");
        assert_eq!(got, Some(payload));
        // Second consume drains to None.
        assert!(store.consume("my-repo", "a30-foo").is_none());
    }

    #[test]
    fn consume_unknown_key_returns_none() {
        let store = SubmissionStore::new();
        assert!(store.consume("my-repo", "never-recorded").is_none());
    }

    #[test]
    fn record_for_occupied_key_replaces_prior_entry() {
        let store = SubmissionStore::new();
        store
            .record("r".into(), "c".into(), "reviewer", json!({"v": 1}))
            .unwrap();
        store
            .record("r".into(), "c".into(), "reviewer", json!({"v": 2}))
            .unwrap();
        assert_eq!(store.consume("r", "c"), Some(json!({"v": 2})));
    }

    #[test]
    fn registered_schema_rejects_invalid_payload_without_storing() {
        let store = SubmissionStore::new();
        // A role whose validator requires a non-empty `verdict` field.
        store.register_schema(
            "reviewer",
            Arc::new(|p: &Value| {
                if p.get("verdict").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty()) {
                    Ok(())
                } else {
                    Err("verdict must be a non-empty string".to_string())
                }
            }),
        );
        let err = store
            .record("r".into(), "c".into(), "reviewer", json!({"verdict": ""}))
            .expect_err("schema-invalid payload must be rejected");
        assert!(err.contains("verdict"), "reason names the field: {err}");
        // Nothing stored.
        assert!(store.consume("r", "c").is_none());
        // A valid payload for the same role round-trips.
        store
            .record("r".into(), "c".into(), "reviewer", json!({"verdict": "approve"}))
            .expect("valid payload accepted");
        assert_eq!(store.consume("r", "c"), Some(json!({"verdict": "approve"})));
    }

    /// verifier-gates-persist-session-log task 5.4: a consume that finds no
    /// live submission can still distinguish "relayed but not consumed" (the
    /// relay reached the daemon — `was_ever_relayed` true even after consume)
    /// from "never relayed" (`was_ever_relayed` false).
    #[test]
    fn was_ever_relayed_distinguishes_relayed_from_never() {
        let store = SubmissionStore::new();
        // Never relayed for this key.
        assert!(
            !store.was_ever_relayed("repo", "never"),
            "a key with no relay must report never-relayed"
        );
        // Relay one, then consume it: the live entry drains to None, but the
        // "was ever relayed" fact persists (so a no-live-submission consume is
        // diagnosable as relayed-but-not-consumed).
        store
            .record("repo".into(), "relayed".into(), "reviewer", json!({"v": 1}))
            .unwrap();
        assert!(store.was_ever_relayed("repo", "relayed"));
        assert!(store.consume("repo", "relayed").is_some(), "live entry present");
        assert!(store.consume("repo", "relayed").is_none(), "drained after consume");
        assert!(
            store.was_ever_relayed("repo", "relayed"),
            "the relayed fact survives consume — distinguishes mode c from mode b"
        );
    }

    /// A schema-REJECTED payload is not stored AND is not marked relayed (the
    /// record short-circuits before either), so a never-corrected rejection is
    /// reported as never-relayed (the daemon stored nothing).
    #[test]
    fn rejected_payload_is_not_marked_relayed() {
        let store = SubmissionStore::new();
        store.register_schema(
            "reviewer",
            Arc::new(|p: &Value| {
                if p.get("verdict").is_some() {
                    Ok(())
                } else {
                    Err("verdict required".to_string())
                }
            }),
        );
        let _ = store.record("r".into(), "c".into(), "reviewer", json!({"bad": true}));
        assert!(
            !store.was_ever_relayed("r", "c"),
            "a rejected payload is not marked relayed"
        );
    }

    /// verifier-gates-persist-session-log task 4.1: the advertised-tool record
    /// distinguishes "a submit tool was advertised" from "none advertised for
    /// the role", survives `consume`, and is `None` for a key that never
    /// recorded an advertisement — so a no-submission `consume` can report mode
    /// (a). Mirrors `was_ever_relayed_distinguishes_relayed_from_never`.
    #[test]
    fn advertised_tool_distinguishes_some_none_and_unrecorded() {
        let store = SubmissionStore::new();
        // No advertisement recorded for this key.
        assert_eq!(
            store.advertised_tool("repo", "never"),
            None,
            "a key with no recorded advertisement reports None"
        );
        // Role with a matching submit tool.
        store.record_advertised_tool(
            "repo".into(),
            "with-tool".into(),
            "reviewer".into(),
            Some("submit_review".into()),
        );
        assert_eq!(
            store.advertised_tool("repo", "with-tool"),
            Some(("reviewer".into(), Some("submit_review".into()))),
            "an advertised tool is reported with its role"
        );
        // Role with NO matching submit tool (mode a fact).
        store.record_advertised_tool(
            "repo".into(),
            "no-tool".into(),
            "implementer".into(),
            None,
        );
        assert_eq!(
            store.advertised_tool("repo", "no-tool"),
            Some(("implementer".into(), None)),
            "a role with no matching tool reports None for the tool"
        );
        // The advertised fact survives consume (the live submission is a
        // separate map; here there is none, so consume drains to None) — the
        // record persists so a no-submission consume can still report mode (a).
        assert!(store.consume("repo", "with-tool").is_none());
        assert_eq!(
            store.advertised_tool("repo", "with-tool"),
            Some(("reviewer".into(), Some("submit_review".into()))),
            "the advertised-tool fact survives consume"
        );
    }

    /// The advertised-tool record is independent of the relayed/submission
    /// state: a session can advertise a tool yet never relay a submission, so a
    /// no-submission consume reports advertised=Some AND relayed=false (mode b
    /// — the tool WAS available but the model never called it).
    #[test]
    fn advertised_and_relayed_are_independent_facts() {
        let store = SubmissionStore::new();
        store.record_advertised_tool(
            "repo".into(),
            "held".into(),
            "reviewer".into(),
            Some("submit_review".into()),
        );
        // No submission was ever relayed for this key.
        assert!(!store.was_ever_relayed("repo", "held"));
        assert!(store.consume("repo", "held").is_none());
        // Both facts are reportable together: advertised but never relayed.
        assert_eq!(
            store.advertised_tool("repo", "held"),
            Some(("reviewer".into(), Some("submit_review".into())))
        );
        assert!(!store.was_ever_relayed("repo", "held"));
    }

    #[test]
    fn keys_do_not_collide_across_repos() {
        let store = SubmissionStore::new();
        store.record("a".into(), "c".into(), "reviewer", json!({"n": "a"})).unwrap();
        store.record("b".into(), "c".into(), "reviewer", json!({"n": "b"})).unwrap();
        assert_eq!(store.consume("a", "c"), Some(json!({"n": "a"})));
        assert_eq!(store.consume("b", "c"), Some(json!({"n": "b"})));
    }
}
