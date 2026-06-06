//! Work lanes (a009).
//!
//! autocoder drives two independent work lanes over the shared per-repo
//! serializer (the busy-marker — one unit of work per repository at a
//! time):
//!
//! - the **changes** lane (`openspec/changes/<slug>/`, carries a spec
//!   delta; lives in `crate::queue` + `crate::polling_loop`), AND
//! - the **issues** lane (`openspec/issues/<slug>/`, a correction that
//!   carries NO delta; this module).
//!
//! The two lanes are driven by SEPARATE walkers, each with its own
//! control flow AND its own state file — NOT one walker with an
//! `is_issue` flag. Lane-specific behavior lives in each walker; the
//! stateless leaf primitives both lanes need (busy-marker, PR opening,
//! archiving, chatops notification, queue-state I/O, AND workspace
//! handling) are composed from [`shared`], which holds a single
//! definition of each rather than a lane-private copy.
//!
//! Lane precedence is `issues > changes > audits` — see [`select`].
//! The issues lane is gated by `features.issues` (off by default) via
//! the task-local context in [`gate`].

pub mod gate;
pub mod ingestion;
pub mod issues;
pub mod select;
pub mod shared;
pub mod state;
pub mod walker;
