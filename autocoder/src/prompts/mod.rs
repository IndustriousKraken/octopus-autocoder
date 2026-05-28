//! Uniform prompt-template loader (a24). Every embedded prompt in
//! `prompts/*.md` is loaded through [`PromptLoader::load`] so the
//! precedence is identical across audit, executor, reviewer, and
//! brownfield consumers: per-workspace nested override → per-workspace
//! flat-legacy override → daemon-level flat-legacy override → embedded
//! default. Missing-override paths log a one-shot WARN naming the
//! `(PromptId, path)` pair.

pub mod loader;

pub use loader::{PromptId, PromptLoader};
