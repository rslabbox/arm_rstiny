//! Lazy EL0 FP/SIMD ownership and context migration.
//!
//! The architecture never restores FP/SIMD state on entry: `CPACR_EL1` traps
//! every EL0 FP/SIMD instruction, and each incoming task pays its first-access
//! trap (`ESR_EL1.EC = 0x07`) before its state is installed. Only one task can
//! own the hardware at a time (this is a single-CPU kernel); on a trap from a
//! different task the previous owner's hardware registers are saved into its
//! `UserContext` box and the trapper's own state is loaded. Since `ELR` is not
//! advanced, the trapping instruction replays and the loop re-enters. Tasks
//! that never touch FP/SIMD never pay anything, and between `eret`s the kernel
//! itself runs with FP/SIMD trapped.
//!
//! Task IDs include a generation, and ownership is cleared by the scheduler
//! (`forget`) before any execution is reclaimed; `migrate` additionally
//! asserts the invariant `owner id != current id` with a debug assert.
//! Design: docs/fpu.md.

use crate::{arch::machine::instructions, utils::single_core::SingleCore};
use aarch64_cpu::registers::{CPACR_EL1, Writeable};
use core::mem::offset_of;

/// Hardware registers whose backing store lives inside each task's
/// `UserContext` box (a `[u128; 32]` vector slice plus the control/status
/// registers). Initialized to zero, matching the architectural reset state of
/// the registers, so a first-time owner sees all-zero state.
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct FpuContext {
    q: [u128; 32],
    fpcr: u64,
    fpsr: u64,
}

const _: () = {
    // 528 = 32 vector registers * 16 bytes + fpcr (8) + fpsr (8).
    assert!(core::mem::size_of::<FpuContext>() == 528);
    // `stp q` needs 16-byte-aligned destinations, and the context sits inside
    // an `UserContext` box whose alignment `u128` already forces to 16.
    assert!(core::mem::align_of::<FpuContext>() == 16);
};

struct Owner {
    /// Full task ID (generation included); the scheduler clears ownership via
    /// `forget` before any execution is reclaimed.
    id: u64,
    /// Stable address of the owner's `FpuContext` inside its `UserContext` box.
    ctx: *mut FpuContext,
}

struct Fpu {
    owner: Option<Owner>,
}

static FPU: SingleCore<Fpu> = SingleCore::new(Fpu { owner: None });

fn with_fpu<T>(operation: impl FnOnce(&mut Fpu) -> T) -> T {
    assert!(instructions::irq_masked());
    operation(&mut FPU.borrow_mut())
}

/// Enable both EL0 and EL1 FP/SIMD access. The `msr` is followed by an `isb`
/// because the save/restore routines below execute FP instructions directly
/// after it (an `eret`, unlike here, is itself a context-synchronizing event).
#[inline]
fn enable() {
    CPACR_EL1.write(CPACR_EL1::FPEN::TrapNothing);
    // SAFETY: a plain context-synchronizing barrier; no state involved.
    unsafe { core::arch::asm!("isb", options(nomem, nostack)) };
}

/// Trap all FP/SIMD access at both EL0 (the lazy owner decision) and EL1
/// (the kernel's own mutation watchdog). No barrier is needed: nothing below
/// uses FP/SIMD and the next `eret` is a context-synchronizing event.
#[inline]
pub(crate) fn disable() {
    CPACR_EL1.write(CPACR_EL1::FPEN::TrapEl0El1);
}

/// Decide CPACR trapping before the next `eret` to the given task: only the
/// current FPU owner enters EL0 with the hardware state enabled; every other
/// entry traps on its first FP/SIMD instruction and migrates (§`migrate`).
pub(crate) fn activate(cur: *mut FpuContext, id: u64) {
    assert!(instructions::irq_masked());
    let owned = with_fpu(|fpu| {
        matches!(&fpu.owner, Some(owner) if owner.id == id && owner.ctx == cur)
    });
    if owned {
        enable();
    } else {
        disable();
    }
}

/// Handle an EL0 FP/SIMD access trap (`ESR_EL1.EC = 0x07`): save the previous
/// owner's hardware registers into its context, load `cur`'s, and leave the
/// hardware enabled for the replaying task. This is the only kernel window in
/// which FP/SIMD is enabled, so the routines below must not reach code that
/// itself uses FP/SIMD (the kernel is compiled softfloat anyway).
///
/// # Invariants
/// - IRQs are masked and this is the only CPU; no parallel FP activity exists.
/// - The previous owner is not `cur` (`activate` would have enabled the trap
///   otherwise), and its context is alive because `forget` runs before any
///   execution reclaim.
pub(crate) fn migrate(cur: *mut FpuContext, id: u64) {
    assert!(instructions::irq_masked());
    with_fpu(|fpu| {
        let previous = fpu.owner.take();
        // The save/restore below are FP instructions themselves and must run
        // with the traps we just took disabled.
        enable();
        if let Some(owner) = previous {
            debug_assert!(owner.id != id, "the FPU owner cannot trap itself");
            // SAFETY: the previous owner is a distinct, still-alive task whose
            // hardware state is exactly what the registers still hold.
            unsafe { fpu_save(owner.ctx) };
        }
        // SAFETY: `cur` is exclusively borrowed for the duration of this call
        // by the trapping task's run loop.
        unsafe { fpu_restore(cur) };
        fpu.owner = Some(Owner { id, ctx: cur });
    });
}

/// The task `id` is being retired: drop its ownership so a later `migrate`
/// never saves hardware registers into a reclaimed `UserContext` box. A no-op
/// when `id` is not the current owner. Called from the single scheduler
/// retire/destroy point before the execution (and its context) is released.
pub(crate) fn forget(id: u64) {
    assert!(instructions::irq_masked());
    with_fpu(|fpu| {
        if fpu.owner.as_ref().is_some_and(|owner| owner.id == id) {
            fpu.owner = None;
        }
    });
}

/// Save `v0..v31` and `FPCR`/`FPSR` (word 0) into `*dst`. The body enables
/// the `neon` target feature purely so the integrated assembler accepts the
/// `stp q` instructions; the function only passes a pointer, so no FP/SIMD
/// crosses its ABI and the feature is sound here. If the softfloat+neon
/// combination ever becomes a hard error (issue #134375), move this code into
/// a separately assembled object file.
///
/// # Safety
/// `dst` must be a live `FpuContext` aligned to 16 bytes, and FP/SIMD must be
/// enabled (the caller sets CPACR before entering this window).
#[allow(aarch64_softfloat_neon)]
#[target_feature(enable = "neon")]
unsafe extern "C" fn fpu_save(dst: *mut FpuContext) {
    // SAFETY (2024 edition): the asm is the point of the function; see the
    // invariants on `fpu_save` itself.
    unsafe {
        core::arch::asm!(
            "mrs x9, fpcr",
            "mrs x10, fpsr",
            "str x9, [x0, {fpcr}]",
            "str x10, [x0, {fpsr}]",
            "stp q0, q1, [x0, #0]",
            "stp q2, q3, [x0, #32]",
            "stp q4, q5, [x0, #64]",
            "stp q6, q7, [x0, #96]",
            "stp q8, q9, [x0, #128]",
            "stp q10, q11, [x0, #160]",
            "stp q12, q13, [x0, #192]",
            "stp q14, q15, [x0, #224]",
            "stp q16, q17, [x0, #256]",
            "stp q18, q19, [x0, #288]",
            "stp q20, q21, [x0, #320]",
            "stp q22, q23, [x0, #352]",
            "stp q24, q25, [x0, #384]",
            "stp q26, q27, [x0, #416]",
            "stp q28, q29, [x0, #448]",
            "stp q30, q31, [x0, #480]",
            in("x0") dst,
            lateout("x9") _, lateout("x10") _,
            lateout("v0") _, lateout("v1") _, lateout("v2") _, lateout("v3") _, lateout("v4") _, lateout("v5") _, lateout("v6") _, lateout("v7") _, lateout("v8") _, lateout("v9") _, lateout("v10") _, lateout("v11") _, lateout("v12") _, lateout("v13") _, lateout("v14") _, lateout("v15") _, lateout("v16") _, lateout("v17") _, lateout("v18") _, lateout("v19") _, lateout("v20") _, lateout("v21") _, lateout("v22") _, lateout("v23") _, lateout("v24") _, lateout("v25") _, lateout("v26") _, lateout("v27") _, lateout("v28") _, lateout("v29") _, lateout("v30") _, lateout("v31") _,
            fpcr = const offset_of!(FpuContext, fpcr),
            fpsr = const offset_of!(FpuContext, fpsr),
            options(nostack, preserves_flags)
        );
    }
}

/// Restore `v0..v31` and `FPCR`/`FPSR` (word 0) from `*src`. Feature rationale
/// and safety as in [`fpu_save`].
///
/// # Safety
/// `src` must point to initialized `FpuContext` data, and FP/SIMD must be
/// enabled (the caller sets CPACR before entering this window).
#[allow(aarch64_softfloat_neon)]
#[target_feature(enable = "neon")]
unsafe extern "C" fn fpu_restore(src: *const FpuContext) {
    // SAFETY (2024 edition): the asm is the point of the function; see the
    // invariants on `fpu_restore` itself.
    unsafe {
        core::arch::asm!(
            "ldr x9, [x0, {fpcr}]",
            "ldr x10, [x0, {fpsr}]",
            "msr fpcr, x9",
            "msr fpsr, x10",
            "ldp q0, q1, [x0, #0]",
            "ldp q2, q3, [x0, #32]",
            "ldp q4, q5, [x0, #64]",
            "ldp q6, q7, [x0, #96]",
            "ldp q8, q9, [x0, #128]",
            "ldp q10, q11, [x0, #160]",
            "ldp q12, q13, [x0, #192]",
            "ldp q14, q15, [x0, #224]",
            "ldp q16, q17, [x0, #256]",
            "ldp q18, q19, [x0, #288]",
            "ldp q20, q21, [x0, #320]",
            "ldp q22, q23, [x0, #352]",
            "ldp q24, q25, [x0, #384]",
            "ldp q26, q27, [x0, #416]",
            "ldp q28, q29, [x0, #448]",
            "ldp q30, q31, [x0, #480]",
            in("x0") src,
            lateout("x9") _, lateout("x10") _,
            lateout("v0") _, lateout("v1") _, lateout("v2") _, lateout("v3") _, lateout("v4") _, lateout("v5") _, lateout("v6") _, lateout("v7") _, lateout("v8") _, lateout("v9") _, lateout("v10") _, lateout("v11") _, lateout("v12") _, lateout("v13") _, lateout("v14") _, lateout("v15") _, lateout("v16") _, lateout("v17") _, lateout("v18") _, lateout("v19") _, lateout("v20") _, lateout("v21") _, lateout("v22") _, lateout("v23") _, lateout("v24") _, lateout("v25") _, lateout("v26") _, lateout("v27") _, lateout("v28") _, lateout("v29") _, lateout("v30") _, lateout("v31") _,
            fpcr = const offset_of!(FpuContext, fpcr),
            fpsr = const offset_of!(FpuContext, fpsr),
            options(nostack, preserves_flags)
        );
    }
}