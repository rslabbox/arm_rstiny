//! Fixed-capacity priority-ready ring: no allocation on the scheduling path.
//!
//! One ring, highest priority first; entries of equal priority keep their
//! enqueue order, so a preempted or yielding task — re-pushed at the tail —
//! round-robins against its peers (docs/roadmap-next.md P1.1). Priorities are
//! re-evaluated on every pop, so a `TcbSetPriority` on a queued task takes
//! effect at the next scheduling point without re-queuing.
pub const MAX_TASKS: usize = 32;
pub struct RunQueue {
    entries: [usize; MAX_TASKS],
    head: usize,
    len: usize,
}
impl RunQueue {
    pub const fn new() -> Self {
        Self {
            entries: [0; MAX_TASKS],
            head: 0,
            len: 0,
        }
    }
    pub fn push(&mut self, task: usize) {
        assert!(self.len < MAX_TASKS);
        self.entries[(self.head + self.len) % MAX_TASKS] = task;
        self.len += 1;
    }
    pub fn pop(&mut self) -> Option<usize> {
        if self.len == 0 {
            return None;
        }
        let task = self.entries[self.head];
        self.head = (self.head + 1) % MAX_TASKS;
        self.len -= 1;
        Some(task)
    }
    /// Remove and return the highest-priority entry, earliest first among
    /// equals. Two rotations of at most [`MAX_TASKS`] entries; the scan pass
    /// leaves the ring exactly as it found it, so equal-priority order is
    /// preserved into the removal pass.
    pub fn pop_best(&mut self, priority: impl Fn(usize) -> u8) -> Option<usize> {
        let count = self.len;
        let mut best: Option<(usize, u8)> = None;
        for _ in 0..count {
            let entry = self.pop()?;
            let level = priority(entry);
            if best.is_none_or(|(_, best_level)| level > best_level) {
                best = Some((entry, level));
            }
            self.push(entry);
        }
        let (task, _) = best?;
        self.remove(task);
        Some(task)
    }
    pub fn remove(&mut self, task: usize) {
        let count = self.len;
        for _ in 0..count {
            let entry = self.pop().unwrap();
            if entry != task {
                self.push(entry);
            }
        }
    }
}
