//! Verifier-gate framework (a61).
//!
//! autocoder runs a set of LLM-driven, change-lifecycle consistency checks
//! around the executor. a61 organizes them as a framework of exactly three
//! named gates positioned around the executor run:
//!
//! - the `[in]` gate — change-internal consistency, run BEFORE the executor
//!   (IS a59's change-internal contradiction pre-flight);
//! - the `[canon]` gate — change-vs-canonical consistency, run BEFORE the
//!   executor (realized by a62);
//! - the `[out]` gate — code-implements-spec, run AFTER the executor
//!   (realized by a63).
//!
//! This module owns ONLY the shared vocabulary: the gate identifiers, their
//! lifecycle positions, the stable diagnostic label each gate's findings
//! carry (`[verifier:in]` / `[verifier:canon]` / `[verifier:out]`), AND the
//! registry that maps a gate identifier to its installed implementation. a61
//! realizes ONLY the `[in]` gate — it maps to the a59 contradiction check;
//! `[canon]` and `[out]` are in the vocabulary but resolve to no installed
//! gate until a62/a63 register them. The framework invokes nothing for an
//! unrealized gate; no gate is run speculatively.
//!
//! This is a deliberately thin, low-behavior-change reframe: it changes
//! NOTHING about what the `[in]` gate decides, its config key, or its alert
//! category — it only gives the existing check a stable identity within the
//! framework so a62/a63 plug into an established frame.

use std::collections::BTreeMap;

/// Where a gate runs relative to the executor. The `[in]` and `[canon]`
/// gates run BEFORE the executor (fail-open posture — a gate's own failure
/// never blocks the iteration); the `[out]` gate runs AFTER (advisory
/// posture — it annotates operator surfaces, it never auto-acts).
#[cfg_attr(not(test), allow(dead_code))] // queried by a62/a63 when they realize [canon]/[out].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecyclePosition {
    /// Runs before the executor; fail-open.
    PreExecutor,
    /// Runs after the executor; advisory.
    PostExecutor,
}

/// One of exactly three named verifier gates positioned around the executor.
/// The identifier (`in` / `canon` / `out`) is stable: it keys the registry
/// AND forms the diagnostic label so a finding is attributable to the gate
/// that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VerifierGate {
    /// Change-internal consistency, pre-executor. IS the a59 change-internal
    /// contradiction pre-flight check.
    In,
    /// Change-vs-canonical consistency, pre-executor. Realized by a62.
    Canon,
    /// Code-implements-spec, post-executor. Realized by a63.
    Out,
}

impl VerifierGate {
    /// Every gate in the fixed vocabulary, in lifecycle order.
    #[cfg_attr(not(test), allow(dead_code))] // iterated by tests and by a62/a63 startup wiring.
    pub const ALL: [VerifierGate; 3] = [VerifierGate::In, VerifierGate::Canon, VerifierGate::Out];

    /// The gate's stable identifier (`in` / `canon` / `out`). Keys the
    /// registry AND forms the diagnostic label.
    pub const fn id(self) -> &'static str {
        match self {
            VerifierGate::In => "in",
            VerifierGate::Canon => "canon",
            VerifierGate::Out => "out",
        }
    }

    /// Where this gate runs relative to the executor.
    #[cfg_attr(not(test), allow(dead_code))] // dispatched on by a62/a63 when they realize their gates.
    pub const fn position(self) -> LifecyclePosition {
        match self {
            VerifierGate::In | VerifierGate::Canon => LifecyclePosition::PreExecutor,
            VerifierGate::Out => LifecyclePosition::PostExecutor,
        }
    }

    /// The gate's stable diagnostic label, e.g. `[verifier:in]`. The shared
    /// labeling token (task 1.2) that prefixes a gate's diagnostics so a
    /// finding is attributable to the gate that produced it.
    pub fn label(self) -> String {
        format!("[verifier:{}]", self.id())
    }

    /// Prefix one log/diagnostic line with this gate's stable label. Callers
    /// build their message AND pass it here so every gate diagnostic — log
    /// line OR operator surface — is uniformly attributable to its gate.
    pub fn label_line(self, line: &str) -> String {
        format!("{} {}", self.label(), line)
    }
}

/// The concrete check an installed gate runs. a61 realizes ONLY
/// [`GateImpl::ContradictionCheck`] (the a59 change-internal contradiction
/// pre-flight, reframed as the `[in]` gate). a62/a63 add variants as they
/// realize the `[canon]` / `[out]` gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateImpl {
    /// The change-internal contradiction pre-flight check (a59). Entry point:
    /// [`crate::preflight::change_contradiction::run_agentic_contradiction_check`].
    ContradictionCheck,
}

/// Maps a [`VerifierGate`] to its installed [`GateImpl`]. A gate that is NOT
/// in the map is unrealized: resolving it yields "no installed gate" AND the
/// framework invokes nothing for it (no gate is run speculatively).
///
/// [`GateRegistry::standard`] is the daemon's registry as of a61 — only the
/// `[in]` gate is installed. a62/a63 [`register`](GateRegistry::register)
/// their gates onto the same registry.
#[derive(Debug, Default, Clone)]
pub struct GateRegistry {
    installed: BTreeMap<VerifierGate, GateImpl>,
}

impl GateRegistry {
    /// The daemon's standard registry as of a61: the `[in]` gate is installed
    /// (mapped to the a59 contradiction check); `[canon]` and `[out]` are in
    /// the vocabulary but unrealized (no installed gate). a62/a63 extend this
    /// by registering their gates.
    pub fn standard() -> Self {
        let mut reg = GateRegistry::default();
        reg.register(VerifierGate::In, GateImpl::ContradictionCheck);
        reg
    }

    /// Install (or replace) the implementation for a gate. a62/a63 call this
    /// to realize the `[canon]` / `[out]` gates.
    pub fn register(&mut self, gate: VerifierGate, gate_impl: GateImpl) {
        self.installed.insert(gate, gate_impl);
    }

    /// Resolve a gate to its installed implementation, OR `None` when the
    /// gate is unrealized ("no installed gate"). The framework invokes
    /// nothing for a `None` resolution.
    pub fn resolve(&self, gate: VerifierGate) -> Option<GateImpl> {
        self.installed.get(&gate).copied()
    }

    /// Whether a gate has an installed implementation.
    #[cfg_attr(not(test), allow(dead_code))] // convenience predicate; used by tests and a62/a63.
    pub fn is_installed(&self, gate: VerifierGate) -> bool {
        self.installed.contains_key(&gate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- gate vocabulary: identifiers, positions, labels ----

    #[test]
    fn ids_are_stable_and_distinct() {
        assert_eq!(VerifierGate::In.id(), "in");
        assert_eq!(VerifierGate::Canon.id(), "canon");
        assert_eq!(VerifierGate::Out.id(), "out");
    }

    #[test]
    fn lifecycle_positions_match_the_framework() {
        // Two pre-executor gates, one post-executor gate.
        assert_eq!(VerifierGate::In.position(), LifecyclePosition::PreExecutor);
        assert_eq!(VerifierGate::Canon.position(), LifecyclePosition::PreExecutor);
        assert_eq!(VerifierGate::Out.position(), LifecyclePosition::PostExecutor);
    }

    #[test]
    fn labels_carry_the_stable_identifier() {
        assert_eq!(VerifierGate::In.label(), "[verifier:in]");
        assert_eq!(VerifierGate::Canon.label(), "[verifier:canon]");
        assert_eq!(VerifierGate::Out.label(), "[verifier:out]");
    }

    #[test]
    fn label_line_prefixes_the_message() {
        let line = VerifierGate::In.label_line("session failed (fail-open)");
        assert_eq!(line, "[verifier:in] session failed (fail-open)");
        // The identifier leads the line so the finding is attributable.
        assert!(line.starts_with("[verifier:in]"));
    }

    #[test]
    fn all_lists_exactly_the_three_gates() {
        assert_eq!(
            VerifierGate::ALL,
            [VerifierGate::In, VerifierGate::Canon, VerifierGate::Out]
        );
    }

    // ---- registry: the [in] gate is installed; [canon]/[out] are inert ----

    /// Task 2.2: the `[in]` gate is resolvable by name to the contradiction
    /// check entry point.
    #[test]
    fn standard_registry_installs_only_the_in_gate() {
        let reg = GateRegistry::standard();
        assert_eq!(
            reg.resolve(VerifierGate::In),
            Some(GateImpl::ContradictionCheck),
            "the [in] gate must map to the a59 contradiction check"
        );
        assert!(reg.is_installed(VerifierGate::In));
    }

    /// Task 3.1 / 4.2: an unrealized gate resolves to "no installed gate" and
    /// the framework invokes nothing for it.
    #[test]
    fn unrealized_gates_resolve_to_no_installed_gate() {
        let reg = GateRegistry::standard();
        assert_eq!(reg.resolve(VerifierGate::Canon), None, "[canon] is unrealized in a61");
        assert_eq!(reg.resolve(VerifierGate::Out), None, "[out] is unrealized in a61");
        assert!(!reg.is_installed(VerifierGate::Canon));
        assert!(!reg.is_installed(VerifierGate::Out));
    }

    /// The registry is extensible: a62/a63 register their gates onto the same
    /// registry, exactly as `standard()` registers the `[in]` gate.
    #[test]
    fn register_realizes_a_previously_inert_gate() {
        let mut reg = GateRegistry::standard();
        assert!(!reg.is_installed(VerifierGate::Canon));
        reg.register(VerifierGate::Canon, GateImpl::ContradictionCheck);
        assert!(reg.is_installed(VerifierGate::Canon));
        assert_eq!(reg.resolve(VerifierGate::Canon), Some(GateImpl::ContradictionCheck));
    }

    #[test]
    fn empty_registry_installs_nothing() {
        let reg = GateRegistry::default();
        for gate in VerifierGate::ALL {
            assert_eq!(reg.resolve(gate), None, "{gate:?} must be inert in an empty registry");
        }
    }
}
