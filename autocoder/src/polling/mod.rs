//! Polling-loop submodules for chat-driven flows. Each submodule owns
//! the "process one queue entry" logic invoked from `polling_loop::run`.

pub mod brownfield;
pub mod brownfield_batch;
pub mod brownfield_survey;
pub mod scout;
pub mod spec_it;
pub mod sync_upstream;
