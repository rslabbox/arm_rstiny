//! Endpoint and Notification payloads.
//!
//! seL4 semantics: an endpoint is a rendezvous point; a notification is a
//! merging bit set. Queues store task identities in a bounded array: every
//! task appears at most once, entries naming finished tasks are pruned lazily
//! by validation, and the payload stays plain data owned by the object table.
use crate::task::queue::MAX_TASKS;
use kernel_abi::NO_MEMORY;

/// FIFO of task identities. Entries may outlive their task; consumers skip
/// and drop identities the scheduler no longer recognises.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WaitQueue {
    ids: [u64; MAX_TASKS],
    len: usize,
}
impl WaitQueue {
    pub const EMPTY: Self = Self {
        ids: [0; MAX_TASKS],
        len: 0,
    };
    pub fn push(&mut self, id: u64) -> Result<(), u64> {
        if self.len >= MAX_TASKS {
            return Err(NO_MEMORY);
        }
        self.ids[self.len] = id;
        self.len += 1;
        Ok(())
    }
    /// The oldest entry, without validating it.
    pub fn peek(&self) -> Option<u64> {
        (self.len > 0).then_some(self.ids[0])
    }
    pub fn pop(&mut self) -> Option<u64> {
        if self.len == 0 {
            return None;
        }
        let id = self.ids[0];
        self.ids.copy_within(1.., 0);
        self.len -= 1;
        Some(id)
    }
    /// Keep only the entries satisfying `keep`; makes room before a push.
    pub fn retain(&mut self, mut keep: impl FnMut(u64) -> bool) {
        let mut write = 0;
        for read in 0..self.len {
            let id = self.ids[read];
            if keep(id) {
                self.ids[write] = id;
                write += 1;
            }
        }
        self.len = write;
    }
}

pub(crate) struct Endpoint {
    pub queue: WaitQueue,
}
impl Endpoint {
    pub const fn new() -> Self {
        Self {
            queue: WaitQueue::EMPTY,
        }
    }
}

pub(crate) struct Notification {
    /// Union of undelivered badges. A signal with a waiter delivers directly
    /// and never touches `bits`.
    pub bits: u64,
    pub queue: WaitQueue,
}
impl Notification {
    pub const fn new() -> Self {
        Self {
            bits: 0,
            queue: WaitQueue::EMPTY,
        }
    }
}
