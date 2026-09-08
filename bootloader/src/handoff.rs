//! Validated boot metadata and transfer to the kernel's high-address entry.
use crate::{console::Console, mmu};
use core::{arch::asm, fmt::Write};

use crate::boot_info::Handoff;

/// # Safety
/// Images and handoff addresses must have passed LoadPlan's validation. Entry
/// must be on the boot CPU at EL1 with MMU/caches off and interrupts masked.
pub(crate) unsafe fn enter(info: Handoff) -> ! {
    // SAFETY: Startup established the EL1 environment and zeroed BSS;
    // LoadPlan validated all loaded destinations before making any RAM writes.
    unsafe {
        mmu::init_boot_page_tables(info.kernel_mapping);
        mmu::enable_mmu();
    }
    let _ = writeln!(
        Console,
        "kernel entry={:#x}; kernel paddr={:#x}; root paddr={:#x}..{:#x}",
        info.kernel_entry,
        info.kernel_mapping.physical().start(),
        info.image_start,
        info.image_end
    );
    let _ = write!(Console, "Enabling MMU and jumping to entry point...\n\n");
    // SAFETY: The high kernel entry and image/DTB ranges are now mapped.
    unsafe { jump_to_kernel(info) }
}

/// Transfer ownership to the kernel using the seL4 ARM six-register contract.
///
/// # Safety
/// The validated kernel entry must be executable with MMU/caches on. Handoff
/// memory must remain valid until the kernel adopts it; this call never returns.
unsafe fn jump_to_kernel(info: Handoff) -> ! {
    // SAFETY: The caller established the kernel mapping and validated metadata.
    // No Rust call follows the branch; x0..x5 carry the boot protocol arguments.
    unsafe {
        asm!(
            "br x6",
            in("x0") info.image_start,
            in("x1") info.image_end,
            in("x2") info.offset,
            in("x3") info.root_entry,
            in("x4") info.dtb,
            in("x5") info.dtb_size,
            in("x6") info.kernel_entry,
            options(noreturn),
        );
    }
}
