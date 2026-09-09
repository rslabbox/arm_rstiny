//! Saved register state, kernel continuation switching and returning EL0 entry.
mod context;
pub(crate) mod kernel_context;
pub(crate) mod user;
pub(crate) use context::TrapFrame;
