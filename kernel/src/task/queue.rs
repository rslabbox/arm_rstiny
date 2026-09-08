//! Fixed-capacity FIFO: no allocation on the scheduling path.
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
