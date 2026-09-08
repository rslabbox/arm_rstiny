//! Shared state for the single-core, IRQ-masked kernel execution domain.
//! No waiting or atomics: recursive mutable access fails immediately.
use core::{
    cell::{Cell, UnsafeCell},
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

#[repr(C)]
pub(crate) struct SingleCore<T> {
    // Preserve the address of debugger-visible state such as SCHEDULER.
    value: UnsafeCell<T>,
    borrowed: Cell<bool>,
}

// SAFETY: only CPU 0 executes kernel code, and all accesses require masked IRQs.
// Kernel paths must not enable IRQs, switch contexts, or enter idle while borrowed.
// FIQ/NMI paths must not access this state. This type is not suitable for SMP.
unsafe impl<T: Send> Sync for SingleCore<T> {}

impl<T> SingleCore<T> {
    pub(crate) const fn new(value: T) -> Self {
        Self {
            value: UnsafeCell::new(value),
            borrowed: Cell::new(false),
        }
    }

    pub(crate) fn borrow_mut(&self) -> Borrow<'_, T> {
        self.try_borrow_mut()
            .expect("recursive kernel state access")
    }

    pub(crate) fn try_borrow_mut(&self) -> Option<Borrow<'_, T>> {
        assert!(
            crate::arch::irq::masked(),
            "kernel state requires masked IRQs"
        );
        if self.borrowed.replace(true) {
            return None;
        }
        Some(Borrow {
            state: self,
            _local: PhantomData,
        })
    }
}

pub(crate) struct Borrow<'a, T> {
    state: &'a SingleCore<T>,
    _local: PhantomData<*mut ()>,
}
impl<T> Deref for Borrow<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the borrow flag guarantees exclusive access until Drop.
        unsafe { &*self.state.value.get() }
    }
}
impl<T> DerefMut for Borrow<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the active borrow is exclusive and this guard is mutable.
        unsafe { &mut *self.state.value.get() }
    }
}
impl<T> Drop for Borrow<'_, T> {
    fn drop(&mut self) {
        self.state.borrowed.set(false);
    }
}
