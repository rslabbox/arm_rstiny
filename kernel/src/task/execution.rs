//! Own a stable entry closure, kernel continuation and private stack.
use super::stack::KernelStack;
use crate::{arch::kernel_context::KernelContext, memory::Error};
use alloc::{alloc::alloc, boxed::Box};
use core::{alloc::Layout, ptr::NonNull};

struct Entry {
    run: Box<dyn FnMut() + Send>,
}
pub(super) struct Execution {
    pub context: KernelContext,
    entry: NonNull<Entry>,
    _stack: KernelStack,
}
// SAFETY: uniquely owned entry and stack, only executed by the single CPU.
unsafe impl Send for Execution {}

fn try_box<T>(value: T) -> Result<Box<T>, Error> {
    if core::mem::size_of::<T>() == 0 {
        return Ok(Box::new(value));
    }
    // SAFETY: allocation matches T; initialize before constructing the owner.
    let pointer = NonNull::new(unsafe { alloc(Layout::new::<T>()) }).ok_or(Error::NoMemory)?;
    unsafe {
        pointer.as_ptr().cast::<T>().write(value);
        Ok(Box::from_raw(pointer.as_ptr().cast::<T>()))
    }
}

impl Execution {
    /// The entry must suspend only at cancellation-safe boundaries: all owned
    /// resources must be in its capture, not in stack locals across suspension.
    /// A killed task's stack is discarded; its capture is dropped by the owner.
    pub fn new(entry: impl FnMut() + Send + 'static) -> Result<Self, Error> {
        let stack = KernelStack::new()?;
        let entry = try_box(Entry {
            run: try_box(entry)?,
        })?;
        let pointer = NonNull::from(Box::leak(entry));
        Ok(Self {
            context: KernelContext::entry(
                stack.top(),
                trampoline as *const () as usize,
                pointer.as_ptr() as usize,
            ),
            entry: pointer,
            _stack: stack,
        })
    }
}
impl Drop for Execution {
    fn drop(&mut self) {
        // SAFETY: the task is no longer executing and can never resume. Its
        // stable capture is reclaimed before the saved kernel stack is freed.
        unsafe { drop(Box::from_raw(self.entry.as_ptr())) };
    }
}

#[unsafe(naked)]
unsafe extern "C" fn trampoline() -> ! {
    core::arch::naked_asm!(
        // The initial context carries the trusted entry pointer in x19.
        "mov x0, x19", "b {entry}",
        entry = sym enter,
    );
}
unsafe extern "C" fn enter(entry: *mut Entry) -> ! {
    // SAFETY: the scheduler keeps the capture alive throughout execution;
    // only this task invokes it. A suspended task cannot be entered twice.
    unsafe { ((*entry).run)() };
    panic!("task entry unexpectedly returned")
}
