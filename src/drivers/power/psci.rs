//! ARM Power State Coordination Interface (PSCI) driver.

#![allow(unused)]
use core::sync::atomic::{AtomicBool, Ordering};
use log::*;

const PSCI_0_2_FN_BASE: u32 = 0x84000000;
const PSCI_0_2_64BIT: u32 = 0x40000000;
const PSCI_0_2_FN_CPU_OFF: u32 = PSCI_0_2_FN_BASE + 2;
const PSCI_0_2_FN_SYSTEM_OFF: u32 = PSCI_0_2_FN_BASE + 8;
const PSCI_0_2_FN_SYSTEM_RESET: u32 = PSCI_0_2_FN_BASE + 9;
const PSCI_0_2_FN64_CPU_ON: u32 = PSCI_0_2_FN_BASE + PSCI_0_2_64BIT + 3;

static PSCI_METHOD_HVC: AtomicBool = AtomicBool::new(false);

/// PSCI return values, inclusive of all PSCI versions.
#[derive(PartialEq, Debug)]
#[repr(i32)]
enum PsciError {
    NotSupported = -1,
    InvalidParams = -2,
    Denied = -3,
    AlreadyOn = -4,
    OnPending = -5,
    InternalFailure = -6,
    NotPresent = -7,
    Disabled = -8,
    InvalidAddress = -9,
}

impl From<i32> for PsciError {
    fn from(code: i32) -> PsciError {
        use PsciError::*;
        match code {
            -1 => NotSupported,
            -2 => InvalidParams,
            -3 => Denied,
            -4 => AlreadyOn,
            -5 => OnPending,
            -6 => InternalFailure,
            -7 => NotPresent,
            -8 => Disabled,
            -9 => InvalidAddress,
            _ => panic!("Unknown PSCI error code: {}", code),
        }
    }
}

/// arm,psci method: smc
fn arm_smccc_smc(func: u32, arg0: usize, arg1: usize, arg2: usize) -> usize {
    let mut ret;
    unsafe {
        core::arch::asm!(
            "smc #0",
            inlateout("x0") func as usize => ret,
            in("x1") arg0,
            in("x2") arg1,
            in("x3") arg2,
        )
    }
    ret
}

/// psci "hvc" method call
fn psci_hvc_call(func: u32, arg0: usize, arg1: usize, arg2: usize) -> usize {
    let ret;
    unsafe {
        core::arch::asm!(
            "hvc #0",
            inlateout("x0") func as usize => ret,
            in("x1") arg0,
            in("x2") arg1,
            in("x3") arg2,
        )
    }
    ret
}

fn psci_call(func: u32, arg0: usize, arg1: usize, arg2: usize) -> Result<(), PsciError> {
    let ret = if PSCI_METHOD_HVC.load(Ordering::Acquire) {
        psci_hvc_call(func, arg0, arg1, arg2)
    } else {
        arm_smccc_smc(func, arg0, arg1, arg2)
    };
    if ret == 0 {
        Ok(())
    } else {
        Err(PsciError::from(ret as i32))
    }
}

/// Halt the current CPU.
#[inline]
pub fn halt() {
    arm_gic::irq_disable();
    aarch64_cpu::asm::wfi(); // should never return
}

/// Initialize with the given PSCI method.
///
/// Method should be either "smc" or "hvc".
pub fn init(method: &str) {
    match method {
        "smc" => PSCI_METHOD_HVC.store(false, Ordering::Release),
        "hvc" => PSCI_METHOD_HVC.store(true, Ordering::Release),
        _ => panic!("Unknown PSCI method: {}", method),
    }
}

/// Shutdown the whole system, including all CPUs.
pub fn system_off() -> ! {
    info!("Shutting down...");
    psci_call(PSCI_0_2_FN_SYSTEM_OFF, 0, 0, 0).ok();
    warn!("It should shutdown!");
    loop {
        halt();
    }
}

/// Power up a core.
pub fn cpu_on(target_cpu: usize, entry_point: usize, arg: usize) {
    info!("Starting CPU {:x} ON ...", target_cpu);
    let res = psci_call(PSCI_0_2_FN64_CPU_ON, target_cpu, entry_point, arg);
    if let Err(e) = res {
        error!("failed to boot CPU {:x} ({:?})", target_cpu, e);
    }
}

/// Power down the calling core.
pub fn cpu_off() {
    const PSCI_POWER_STATE_TYPE_POWER_DOWN: u32 = 1;
    const PSCI_0_2_POWER_STATE_TYPE_SHIFT: u32 = 16;
    let state: u32 = PSCI_POWER_STATE_TYPE_POWER_DOWN << PSCI_0_2_POWER_STATE_TYPE_SHIFT;
    psci_call(PSCI_0_2_FN_CPU_OFF, state as usize, 0, 0).ok();
}
