use super::*;
const MAX_COPY: usize = 4096;

impl Scheduler {
    pub(super) fn syscall(&mut self, caller: usize) {
        let registers = self.tasks[caller].context.r;
        let args = [
            registers[0],
            registers[1],
            registers[2],
            registers[3],
            registers[4],
        ];
        let result = self.invoke(caller, registers[8], args);
        match result {
            Ok(value) => {
                self.tasks[caller].context.r[0] = OK;
                if let Some(value) = value {
                    self.tasks[caller].context.r[1] = value;
                }
            }
            Err(code) => self.tasks[caller].context.r[0] = code,
        }
    }
    fn space(&self, slot: usize) -> Result<&AddressSpace, u64> {
        self.tasks[slot].space.as_ref().ok_or(INVALID_STATE)
    }
    fn space_mut(&mut self, slot: usize) -> Result<&mut AddressSpace, u64> {
        self.tasks[slot].space.as_mut().ok_or(INVALID_STATE)
    }
    fn invoke(&mut self, caller: usize, number: u64, a: [u64; 5]) -> Result<Option<u64>, u64> {
        let result = match number {
            SYS_YIELD => None,
            SYS_DEBUG_PUTCHAR => {
                if a[0] > 255 {
                    return Err(INVALID_ARGUMENT);
                }
                if log::max_level() == log::LevelFilter::Off {
                    return Err(UNSUPPORTED);
                }
                crate::utils::logging::debug_putchar(a[0] as u8);
                None
            }
            SYS_TASK_ID => Some(self.tasks[caller].id),
            SYS_TASK_CREATE => {
                let space = AddressSpace::new().map_err(|e| e as u64)?;
                let slot = self.create(self.tasks[caller].id, space)?;
                Some(self.tasks[slot].id)
            }
            SYS_SUSPEND_SELF => {
                self.tasks[caller].suspended_from = TASK_RUNNING;
                self.tasks[caller].state = TASK_SUSPENDED;
                None
            }
            SYS_EXIT => {
                self.finish(caller, false, a[0]);
                None
            }
            SYS_SLEEP => {
                let duration = a[0].checked_mul(irq::frequency()).ok_or(INVALID_ARGUMENT)? / 1000;
                self.tasks[caller].deadline =
                    irq::now().checked_add(duration).ok_or(INVALID_ARGUMENT)?;
                self.tasks[caller].state = TASK_SLEEPING;
                None
            }
            SYS_CLOCK => Some(irq::now() / (irq::frequency() / 1000).max(1)),
            SYS_MEMORY_AVAILABLE => Some(memory::available_frames() as u64),
            SYS_TASK_START => {
                let slot = self.lookup(caller, a[0])?;
                if self.tasks[slot].state != TASK_CREATED {
                    return Err(INVALID_STATE);
                }
                if a[1] & 3 != 0 || a[2] & 15 != 0 {
                    return Err(INVALID_ARGUMENT);
                }
                self.space(slot)?
                    .check(a[1] as usize, 4, 4)
                    .map_err(|e| e as u64)?;
                let stack = a[2].checked_sub(16).ok_or(INVALID_ARGUMENT)?;
                self.space(slot)?
                    .check(stack as usize, 16, 2)
                    .map_err(|e| e as u64)?;
                self.tasks[slot].context = TrapFrame::user(a[1], a[2], a[3]);
                self.tasks[slot].started = true;
                self.ready(slot);
                None
            }
            SYS_TASK_STATUS => {
                let slot = self.lookup(caller, a[0])?;
                Some(self.tasks[slot].state)
            }
            SYS_TASK_SUSPEND => {
                let slot = self.lookup(caller, a[0])?;
                if !self.tasks[slot].started || self.tasks[slot].terminal() {
                    return Err(INVALID_STATE);
                }
                self.queue.remove(slot);
                if self.tasks[slot].state == TASK_SUSPENDED {
                    return Err(INVALID_STATE);
                }
                self.tasks[slot].suspended_from = self.tasks[slot].state;
                self.tasks[slot].state = TASK_SUSPENDED;
                None
            }
            SYS_TASK_RESUME => {
                let slot = self.lookup(caller, a[0])?;
                if self.tasks[slot].state != TASK_SUSPENDED {
                    return Err(INVALID_STATE);
                }
                match self.tasks[slot].suspended_from {
                    TASK_WAITING => self.tasks[slot].state = TASK_WAITING,
                    TASK_SLEEPING if irq::now() < self.tasks[slot].deadline => {
                        self.tasks[slot].state = TASK_SLEEPING
                    }
                    _ => self.ready(slot),
                }
                None
            }
            SYS_TASK_DESTROY => {
                let slot = self.lookup(caller, a[0])?;
                if slot == caller {
                    return Err(INVALID_ARGUMENT);
                }
                if !self.tasks[slot].terminal() {
                    self.finish(slot, false, u64::MAX);
                }
                self.tasks[slot] = Task::empty();
                None
            }
            SYS_WAIT => {
                let slot = self.lookup(caller, a[0])?;
                if slot == caller {
                    return Err(INVALID_ARGUMENT);
                }
                if self.tasks[slot].terminal() {
                    Some(self.tasks[slot].result)
                } else {
                    self.tasks[caller].state = TASK_WAITING;
                    self.tasks[caller].wait_for = a[0];
                    None
                }
            }
            SYS_MAP | SYS_UNMAP | SYS_PROTECT => {
                let slot = self.lookup(caller, a[0])?;
                // Other tasks must be stopped before their executable state is edited.
                if slot != caller
                    && !matches!(self.tasks[slot].state, TASK_CREATED | TASK_SUSPENDED)
                {
                    return Err(BUSY);
                }
                let space = self.space_mut(slot)?;
                let (va, len) = (a[1] as usize, a[2] as usize);
                match number {
                    SYS_MAP => space.map(va, len, a[3], false),
                    SYS_UNMAP => space.unmap(va, len),
                    _ => space.protect(va, len, a[3]),
                }
                .map_err(|e| e as u64)?;
                None
            }
            SYS_WRITE_MEMORY | SYS_READ_MEMORY => {
                let slot = self.lookup(caller, a[0])?;
                if slot != caller
                    && !matches!(self.tasks[slot].state, TASK_CREATED | TASK_SUSPENDED)
                {
                    return Err(BUSY);
                }
                let len = a[3] as usize;
                if len > MAX_COPY {
                    return Err(INVALID_ARGUMENT);
                }
                let mut buffer = [0u8; MAX_COPY];
                if number == SYS_WRITE_MEMORY {
                    self.space(caller)?
                        .read(a[2] as usize, &mut buffer[..len])
                        .map_err(|e| e as u64)?;
                    self.space_mut(slot)?
                        .write(a[1] as usize, &buffer[..len])
                        .map_err(|e| e as u64)?;
                } else {
                    self.space(slot)?
                        .read(a[1] as usize, &mut buffer[..len])
                        .map_err(|e| e as u64)?;
                    self.space_mut(caller)?
                        .write(a[2] as usize, &buffer[..len])
                        .map_err(|e| e as u64)?;
                }
                None
            }
            _ => return Err(UNSUPPORTED),
        };
        Ok(result)
    }
}
