//! Kernel-facing user API: dispatch, messages, debug calls and fault handling.
mod debug;
mod dispatch;
pub(crate) mod faults;
mod message;
pub(crate) use dispatch::dispatch;
pub(crate) use message::{Completion, Request};
