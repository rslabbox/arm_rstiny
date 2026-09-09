//! Own a stable entry closure, kernel continuation, saved user frame and
//! private stack.
use super::stack::KernelStack;
use crate::{
    arch::kernel::thread::{kernel_context::KernelContext, user::UserContext},
    memory::Error,
};
use alloc::{alloc::alloc, boxed::Box};
use core::{alloc::Layout, ptr::NonNull};

struct Entry {
    run: Box<dyn FnMut() + Send>,
}
pub(super) struct Execution {
    pub context: KernelContext,
    /// Stable address of the saved user frame. The frame is heap-allocated so
    /// the kernel can deliver IPC into a blocked task's registers.
    frame: NonNull<UserContext>,
    entry: NonNull<Entry>,
    _stack: KernelStack,
}
// SAFETY: uniquely owned entry, frame and stack, only executed by the single CPU.
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
    /// Create an execution around an explicit user frame. The frame pointer
    /// stays valid for the lifetime of the execution and lets the scheduler
    /// mutate a blocked task's registers between runs. The entry must suspend
    /// only at cancellation-safe boundaries: resources owned across a
    /// suspension stay in its capture, never in stack locals.
    pub fn start(
        frame: UserContext,
        mut entry: impl FnMut(&mut UserContext) + Send + 'static,
    ) -> Result<Self, Error> {
        let stack = KernelStack::new()?;
        let frame = NonNull::from(Box::leak(try_box(frame)?));
        // Capture the frame address as `usize`: it is `Send` and copied, so
        // the entry closure stays `FnMut`.
        let address = frame.as_ptr() as usize;
        let entry = try_box(Entry {
            run: try_box(move || {
                // SAFETY: the address is the execution's leaked frame, alive
                // and owned until `Drop`; only this task's loop uses it.
                entry(unsafe { &mut *(address as *mut UserContext) })
            })?,
        })?;
        let entry = NonNull::from(Box::leak(entry));
        Ok(Self {
            context: KernelContext::entry(stack.top(), trampoline, entry.as_ptr() as usize),
            frame,
            entry,
            _stack: stack,
        })
    }
    /// The saved user frame. Only the scheduler may call this, and only while
    /// the task is not executing (blocked or suspended) on the single CPU.
    pub fn frame_mut(&mut self) -> &mut UserContext {
        // SAFETY: the frame outlives the entry closure; exclusive ownership
        // holds because the owning task cannot run while it is being edited.
        unsafe { self.frame.as_mut() }
    }
}
impl Drop for Execution {
    fn drop(&mut self) {
        // SAFETY: the task is no longer executing and can never resume. Its
        // stable capture is reclaimed before the saved kernel stack is freed.
        unsafe {
            drop(Box::from_raw(self.entry.as_ptr()));
            drop(Box::from_raw(self.frame.as_ptr()));
        }
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
