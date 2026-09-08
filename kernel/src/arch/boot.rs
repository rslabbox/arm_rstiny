//! seL4 elfloader handoff and high-half mappings for single-core QEMU virt.
//! User roots retain these supervisor mappings and own their user subtrees.
use aarch64_cpu::{asm::barrier, registers::*};
use core::ptr::{addr_of, addr_of_mut};
use memory_addr::PhysAddr;

use super::PageTableEntry;
use crate::config::{MemFlags, PAGE_SIZE, RAM_START, phys_to_virt, virt_to_phys};

// seL4 ARM loader passes x0..x5 and enters an EL1 kernel with MMU/caches on.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct BootInfo {
    pub image_start: usize,
    pub image_end: usize,
    pub phys_virt_offset: usize,
    pub entry: usize,
    pub dtb: usize,
    pub dtb_size: usize,
}
#[unsafe(no_mangle)]
static mut LOADER_BOOT_INFO: BootInfo = BootInfo {
    image_start: 0,
    image_end: 0,
    phys_virt_offset: 0,
    entry: 0,
    dtb: 0,
    dtb_size: 0,
};
pub fn information() -> BootInfo {
    // SAFETY: initialized once before kernel startup, then immutable on this CPU.
    unsafe { core::ptr::addr_of!(LOADER_BOOT_INFO).read() }
}

#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.boot")]
unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "msr daifset, #0xf",
        "mrs x9, mpidr_el1", "ldr x10, =0xff00ffffff",
        "tst x9, x10", "b.ne 2f",
        "msr spsel, #1", "ldr x9, =boot_stack_top", "mov sp, x9",
        "b {start}", "2:", "wfe", "b 2b", start = sym start_rust,
    );
}

extern "C" fn start_rust(
    image_start: usize,
    image_end: usize,
    phys_virt_offset: usize,
    entry: usize,
    dtb: usize,
    dtb_size: usize,
) -> ! {
    if CurrentEL.read(CurrentEL::EL) != 1
        || !SCTLR_EL1.is_set(SCTLR_EL1::M)
        || !SCTLR_EL1.is_set(SCTLR_EL1::C)
        || !SCTLR_EL1.is_set(SCTLR_EL1::I)
    {
        crate::utils::halt();
    }
    VBAR_EL1.set(exception_vector_base as *const () as u64);
    barrier::isb(barrier::SY);
    let mut cursor = addr_of_mut!(sbss).cast::<u64>();
    let end = addr_of_mut!(ebss).cast::<u64>();
    while cursor < end {
        // SAFETY: linker bounds exclude the active high-address kernel stack.
        unsafe {
            cursor.write_volatile(0);
            cursor = cursor.add(1);
        }
    }
    let info = BootInfo {
        image_start,
        image_end,
        phys_virt_offset,
        entry,
        dtb,
        dtb_size,
    };
    // Check ranges before using any loader-controlled address in a page table.
    let kernel_end = crate::config::virt_to_phys(addr_of!(ekernel) as usize);
    if image_start < kernel_end
        || image_start >= image_end
        || image_end > 0x41ff_f000
        || !image_start.is_multiple_of(PAGE_SIZE)
        || !image_end.is_multiple_of(PAGE_SIZE)
        || image_start.checked_sub(phys_virt_offset) != Some(kernel_abi::IMAGE_START as usize)
        || image_end.checked_sub(phys_virt_offset) != Some(kernel_abi::STACK_END as usize)
        || !(kernel_abi::IMAGE_START as usize..kernel_abi::IMAGE_END as usize).contains(&entry)
        || dtb < kernel_end
        || dtb_size < 40
        || dtb_size > kernel_abi::MAX_DTB_SIZE as usize
        || dtb
            .checked_add(dtb_size)
            .is_none_or(|end| end > image_start)
    {
        crate::utils::halt();
    }
    // SAFETY: IRQs remain masked and no other CPU executes kernel code.
    unsafe {
        addr_of_mut!(LOADER_BOOT_INFO).write(info);
    }
    boot_main(1)
}

#[derive(Clone, Copy)]
#[repr(C, align(4096))]
pub(super) struct Table(pub(super) [PageTableEntry; 512]);
impl Table {
    pub(super) const EMPTY: Self = Self([PageTableEntry::empty(); 512]);
}

static mut ROOT: Table = Table::EMPTY;
static mut EMPTY_USER_ROOT: Table = Table::EMPTY;
static mut RAM_L1: Table = Table::EMPTY;
static mut RAM_L2: Table = Table::EMPTY;
static mut RAM_L3: [Table; 16] = [Table::EMPTY; 16];
static mut DEVICE_L2: Table = Table::EMPTY;
static mut GIC_L3: Table = Table::EMPTY;
static mut DEVICE_L3: Table = Table::EMPTY;

unsafe extern "C" {
    fn exception_vector_base();
    static mut sbss: u8;
    static mut ebss: u8;
    static skernel: u8;
    static etext: u8;
    static erodata: u8;
    static ekernel: u8;
    static stack_guard: u8;
}

// These functions run under loader mappings with interrupts masked. Tables have
// exclusive boot-time ownership; use raw pointers instead of static-mut refs.
unsafe fn set(table: *mut Table, index: usize, entry: PageTableEntry) {
    unsafe {
        addr_of_mut!((*table).0)
            .cast::<PageTableEntry>()
            .add(index)
            .write(entry)
    };
}
fn table_entry(table: *const Table) -> PageTableEntry {
    PageTableEntry::new_table(PhysAddr::from_usize(virt_to_phys(table as usize)))
}

unsafe fn init_page_tables() {
    unsafe {
        set(addr_of_mut!(ROOT), 0, table_entry(addr_of!(RAM_L1)));
        set(addr_of_mut!(RAM_L1), 1, table_entry(addr_of!(RAM_L2)));
        let tables = addr_of_mut!(RAM_L3).cast::<Table>();
        let start = addr_of!(skernel) as usize;
        let end = phys_to_virt(information().image_end + PAGE_SIZE);
        for va in (start..end).step_by(PAGE_SIZE) {
            if va == addr_of!(stack_guard) as usize {
                continue;
            }
            let l2 = (virt_to_phys(va) - RAM_START) >> 21;
            assert!(l2 < 16);
            let l3 = tables.add(l2);
            set(addr_of_mut!(RAM_L2), l2, table_entry(l3));
            let flags = if va < addr_of!(etext) as usize {
                MemFlags::READ | MemFlags::EXECUTE
            } else if va < addr_of!(erodata) as usize {
                MemFlags::READ
            } else {
                MemFlags::READ | MemFlags::WRITE
            };
            set(
                l3,
                (va >> 12) & 511,
                PageTableEntry::new_page(PhysAddr::from_usize(virt_to_phys(va)), flags, false),
            );
        }
        {
            set(
                addr_of_mut!(DEVICE_L2),
                0x0800_0000 >> 21,
                table_entry(addr_of!(GIC_L3)),
            );
            use crate::config::{GICD_BASE, GICD_SIZE, GICR_BASE, GICR_SIZE, PAGE_SIZE};
            for va in (GICD_BASE..GICD_BASE + GICD_SIZE)
                .step_by(PAGE_SIZE)
                .chain((GICR_BASE..GICR_BASE + GICR_SIZE).step_by(PAGE_SIZE))
            {
                set(
                    addr_of_mut!(GIC_L3),
                    (va >> 12) & 511,
                    PageTableEntry::new_page(
                        PhysAddr::from_usize(va),
                        MemFlags::READ | MemFlags::WRITE | MemFlags::DEVICE,
                        false,
                    ),
                );
            }
        }
        {
            // Keep UART mapped even with LOG=off for the panic console.
            let va = virt_to_phys(crate::config::PL011_UART_BASE);
            set(addr_of_mut!(RAM_L1), 0, table_entry(addr_of!(DEVICE_L2)));
            set(
                addr_of_mut!(DEVICE_L2),
                (va >> 21) & 511,
                table_entry(addr_of!(DEVICE_L3)),
            );
            set(
                addr_of_mut!(DEVICE_L3),
                (va >> 12) & 511,
                PageTableEntry::new_page(
                    PhysAddr::from_usize(va),
                    MemFlags::READ | MemFlags::WRITE | MemFlags::DEVICE,
                    false,
                ),
            );
        }
    }
}

unsafe fn enable_mmu() {
    // Keep elfloader's MAIR indices while replacing its live high mapping:
    // Attr0=Device-nGnRnE, Attr4=normal WB. Changing them first is unsafe.
    barrier::dsb(barrier::SY);
    TTBR1_EL1.set(virt_to_phys(addr_of!(ROOT) as usize) as u64);
    TTBR0_EL1.set(virt_to_phys(addr_of!(EMPTY_USER_ROOT) as usize) as u64);
    barrier::isb(barrier::SY);
    super::instructions::flush_tlb_all();
    TCR_EL1.modify(TCR_EL1::SH0::Inner + TCR_EL1::SH1::Inner);
    barrier::isb(barrier::SY);
    SCTLR_EL1.modify(SCTLR_EL1::SA::Enable + SCTLR_EL1::SA0::Enable + SCTLR_EL1::WXN::Enable);
    barrier::isb(barrier::SY);
}

#[unsafe(no_mangle)]
extern "C" fn boot_main(entry_el: u64) -> ! {
    unsafe {
        crate::BOOT_ENTRY_EL.write_volatile(entry_el);
        init_page_tables();
        enable_mmu();
    }
    crate::rust_main()
}

/// Install an empty, private TTBR0 hierarchy. Supervisor mappings live in TTBR1.
/// SAFETY: destinations are distinct writable 4 KiB frames, not active tables.
pub(crate) unsafe fn prepare_user_tables(root: usize, l1: usize, l2: usize) {
    unsafe {
        set(root as *mut Table, 0, table_entry(l1 as *const Table));
        set(l1 as *mut Table, 0, table_entry(l2 as *const Table));
    }
}
pub(crate) fn kernel_root() -> usize {
    virt_to_phys(addr_of!(EMPTY_USER_ROOT) as usize)
}
