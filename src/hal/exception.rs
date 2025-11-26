//! Exception and interrupt handling.

use aarch64_cpu::registers::{ESR_EL1, FAR_EL1};
use aarch64_cpu::registers::{Readable, VBAR_EL1, Writeable};
use spin::Mutex;

use crate::hal::TrapFrame;

core::arch::global_asm!(include_str!("trap.S"));

#[repr(u8)]
#[derive(Debug)]
#[allow(dead_code)]
enum TrapKind {
    Synchronous = 0,
    Irq = 1,
    Fiq = 2,
    SError = 3,
}

#[repr(u8)]
#[derive(Debug)]
#[allow(dead_code)]
enum TrapSource {
    CurrentSpEl0 = 0,
    CurrentSpElx = 1,
    LowerAArch64 = 2,
    LowerAArch32 = 3,
}

#[unsafe(no_mangle)]
fn invalid_exception(tf: &TrapFrame, kind: TrapKind, source: TrapSource) {
    error!(
        "Invalid exception {:?} from {:?}:\n{:#x?}",
        kind, source, tf
    );

    error!("\n{}", tf.backtrace());

    panic!("Invalid exception {:?} from {:?}", kind, source);
}

use core::marker::PhantomData;

// 标记：禁止锁的上下文
pub struct NoLockContext<'a> {
    _phantom: PhantomData<&'a ()>,
}

impl<'a> NoLockContext<'a> {
    // 私有构造函数，只能内部创建
    fn new() -> Self {
        Self {
            _phantom: PhantomData,
        }
    }

    // 执行无锁闭包
    pub fn execute<F, R>(f: F) -> R
    where
        F: FnOnce(&NoLockContext) -> R,
    {
        let ctx = NoLockContext::new();
        f(&ctx)
    }
}

// 只允许无锁操作的 API
impl NoLockContext<'_> {
    // 只提供无锁的原子操作
    pub fn atomic_load(&self, atomic: &core::sync::atomic::AtomicU32) -> u32 {
        atomic.load(core::sync::atomic::Ordering::Relaxed)
    }

    pub fn atomic_store(&self, atomic: &core::sync::atomic::AtomicU32, val: u32) {
        atomic.store(val, core::sync::atomic::Ordering::Relaxed)
    }

    // 不提供任何可能涉及锁的操作
}
use core::sync::atomic::AtomicU32;
static COUNTER: AtomicU32 = AtomicU32::new(0);
static TASK: Mutex<i32> = Mutex::new(0);
pub fn watchdog_handle() {
    // 可以执行 - 只使用原子操作
    NoLockContext::execute(|ctx| {
        let val = ctx.atomic_load(&COUNTER);
        ctx.atomic_store(&COUNTER, val + 1);
    });

    NoLockContext::execute(|_| {
        let mut task = TASK.lock();
        *task += 1;
    });
}

#[unsafe(no_mangle)]
fn handle_irq_exception(_tf: &mut TrapFrame) {
    trace!("handle_irq_exception");
    crate::drivers::irq::gicv3::irq_handler();

    watchdog_handle();

    // After handling interrupt, check if we need to schedule
    if crate::task::is_initialized() {
        crate::task::schedule();
    }
}

fn handle_instruction_abort(tf: &TrapFrame, _iss: u64) {
    panic!(
        "Instruction Abort @ {:#x}, ESR={:#x}, FAR={:#x}:\n{:#x?}",
        tf.elr,
        ESR_EL1.get(),
        FAR_EL1.get(),
        tf,
    );
}

fn handle_data_abort(tf: &TrapFrame, _iss: u64) {
    error!(
        "Data Abort @ {:#x}, ESR={:#x}, FAR={:#x}:\n{:#x?}",
        tf.elr,
        ESR_EL1.get(),
        FAR_EL1.get(),
        tf,
    );

    error!("\n{}", tf.backtrace());

    panic!("Data Abort encountered");
}

#[unsafe(no_mangle)]
fn handle_sync_exception(tf: &mut TrapFrame) {
    let esr = ESR_EL1.extract();
    let iss = esr.read(ESR_EL1::ISS);
    match esr.read_as_enum(ESR_EL1::EC) {
        Some(ESR_EL1::EC::Value::InstrAbortCurrentEL) => handle_instruction_abort(tf, iss),
        Some(ESR_EL1::EC::Value::DataAbortCurrentEL) => handle_data_abort(tf, iss),
        Some(ESR_EL1::EC::Value::Brk64) => {
            debug!("BRK #{:#x} @ {:#x} ", iss, tf.elr);
            tf.elr += 4;
        }
        _ => {
            panic!(
                "Unhandled synchronous exception @ {:#x}: ESR={:#x} (EC {:#08b}, ISS {:#x}), tf ={:#x?}",
                tf.elr,
                esr.get(),
                esr.read(ESR_EL1::EC),
                esr.read(ESR_EL1::ISS),
                tf
            );
        }
    }
}

/// Initializes trap handling on the current CPU.
///
/// In detail, it initializes the exception vector, and sets `TTBR0_EL1` to 0 to
/// block low address access.
pub fn init_exception() {
    unsafe extern "C" {
        fn exception_vector_base();
    }

    VBAR_EL1.set(exception_vector_base as _);
}
