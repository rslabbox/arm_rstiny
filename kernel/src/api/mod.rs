//! Kernel-facing user API: dispatch, messages, IPC, debug calls and faults.
mod debug;
mod dispatch;
pub(crate) mod faults;
mod ipc;
mod message;
pub(crate) use dispatch::dispatch;
pub(crate) use ipc::signal_notification;
pub(crate) use message::{Completion, Request};
