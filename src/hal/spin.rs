use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::hal::cpu::{disable_irqs, enable_irqs, irqs_disabled};
use lock_api::RawMutex;

pub struct SpinNoIrq {
    lock: AtomicBool,
    saved_irq: UnsafeCell<bool>,
}

unsafe impl Sync for SpinNoIrq {}
unsafe impl Send for SpinNoIrq {}

unsafe impl RawMutex for SpinNoIrq {
    type GuardMarker = lock_api::GuardSend;
    const INIT: Self = Self {
        lock: AtomicBool::new(false),
        saved_irq: UnsafeCell::new(false),
    };

    fn lock(&self) {
        let irq_enabled_before = !irqs_disabled();
        disable_irqs();
        while self
            .lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.lock.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
        unsafe { *self.saved_irq.get() = irq_enabled_before };
    }

    fn try_lock(&self) -> bool {
        let irq_enabled_before = !irqs_disabled();
        disable_irqs();
        if self
            .lock
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            unsafe { *self.saved_irq.get() = irq_enabled_before };
            true
        } else {
            if irq_enabled_before {
                enable_irqs();
            }
            false
        }
    }

    unsafe fn unlock(&self) {
        let irq_enabled_before = unsafe { *self.saved_irq.get() };
        self.lock.store(false, Ordering::Release);
        if irq_enabled_before {
            enable_irqs();
        }
    }
}

pub type Mutex<T> = lock_api::Mutex<SpinNoIrq, T>;