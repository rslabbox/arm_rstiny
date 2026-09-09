//! Kernel address-space ownership and mapping policy. Available before the heap.
use crate::memory::address::{phys_to_virt, virt_to_phys};
mod layout;
use crate::{
    arch::{
        self,
        kernel::vspace::{
            PageTableEntry,
            paging::{Mapper, Root, TablePool},
        },
    },
    config::{self, MemFlags, PAGE_SIZE},
    utils::single_core::SingleCore,
};
pub(crate) use arch::kernel::vspace::paging::MapError;
use core::ptr::addr_of;
use layout::{KernelLayout, Region};
use memory_addr::VirtAddr;

// Conservative fixed-platform budget: two RAM-sized windows of 4 KiB pages,
// plus roots/upper levels and separate MMIO branches. Only the required tables
// are allocated. Exhaustion is reported by the mapper, never an unchecked index.
const TABLE_CAPACITY: usize = 2 * (config::RAM_END - config::RAM_START).div_ceil(1 << 21) + 12;
struct KernelSpace {
    tables: TablePool<TABLE_CAPACITY>,
    roots: Option<(Root, Root)>,
}
static SPACE: SingleCore<KernelSpace> = SingleCore::new(KernelSpace {
    tables: TablePool::new(),
    roots: None,
});

fn map_regions(
    mapper: &mut Mapper<'_, TABLE_CAPACITY>,
    regions: impl Iterator<Item = Region>,
) -> Result<(), MapError> {
    for region in regions {
        mapper.map_region(
            region.virtual_start,
            region.physical_start,
            region.size,
            region.flags,
        )?;
    }
    Ok(())
}
fn build_kernel_space(
    mapper: &mut Mapper<'_, TABLE_CAPACITY>,
    layout: &KernelLayout,
) -> Result<(), MapError> {
    map_regions(mapper, layout.image_regions())?;
    map_regions(mapper, layout.physical_regions())?;
    map_regions(mapper, layout.device_regions())
}

/// Build and install the final high mapping, leaving TTBR0 empty.
/// # Safety
/// Call once, on the boot CPU with IRQs masked, under validated loader mappings.
/// BSS must be cleared and boot information published; no allocator is required.
pub(crate) unsafe fn init() {
    let layout =
        KernelLayout::from_boot(arch::kernel::boot::information()).expect("kernel mapping layout");
    let (high, low) = {
        let mut state = SPACE.borrow_mut();
        assert!(
            state.roots.is_none(),
            "kernel address space already initialized"
        );
        let high = state.tables.new_root().expect("kernel root storage");
        let low = state.tables.new_root().expect("empty user root storage");
        build_kernel_space(&mut state.tables.mapper(high), &layout).expect("kernel mappings");
        state.roots = Some((high, low));
        (
            state.tables.root_address(high),
            state.tables.root_address(low),
        )
    };
    // SAFETY: mappings preserve code, stack and stable SPACE storage. The borrow
    // ends before activation; no temporary mutable reference crosses the switch.
    unsafe { arch::machine::mmu::install_kernel_roots(high, low) };
}

pub(crate) fn empty_user_root() -> usize {
    let state = SPACE.borrow_mut();
    state
        .tables
        .root_address(state.roots.expect("kernel mappings initialized").1)
        .as_usize()
}

/// Query the installed kernel root. Address arithmetic alone cannot detect guards.
pub(crate) fn translate(va: VirtAddr) -> Result<super::Translation, MapError> {
    let mut state = SPACE.borrow_mut();
    let root = state.roots.ok_or(MapError::NotMapped)?.0;
    state.tables.mapper(root).translate(va)
}

/// Guard or restore one owned heap page in both kernel aliases.
/// # Safety
/// The caller exclusively owns the page, will not access it while guarded, and
/// restores it before deallocation. No live stack or reference may use the page.
pub(crate) unsafe fn set_heap_guard(address: usize, guarded: bool) {
    unsafe extern "C" {
        static __heap_start: u8;
        static __heap_end: u8;
    }
    assert!(address % PAGE_SIZE == 0);
    assert!((addr_of!(__heap_start) as usize..addr_of!(__heap_end) as usize).contains(&address));
    let pa = virt_to_phys(VirtAddr::from_usize(address)).expect("heap kernel address");
    let image_va = super::address::kernel_image()
        .expect("kernel image")
        .to_virt(pa)
        .expect("heap image alias");
    let direct_va = phys_to_virt(pa).expect("heap direct alias");
    let normal = PageTableEntry::new_page(pa, MemFlags::READ | MemFlags::WRITE, false);
    let expected = if guarded {
        normal
    } else {
        PageTableEntry::empty()
    };
    let replacement = if guarded {
        PageTableEntry::empty()
    } else {
        normal
    };
    {
        let mut state = SPACE.borrow_mut();
        let root = state.roots.expect("kernel mappings initialized").0;
        let mut mapper = state.tables.mapper(root);
        let image = mapper.leaf(image_va).expect("heap image table");
        let direct = mapper.leaf(direct_va).expect("heap direct table");
        // Validate both aliases before changing either; no table allocation or
        // reclamation is allowed here, including on guard restoration.
        assert_eq!(
            mapper.read(image),
            expected,
            "unexpected heap image mapping"
        );
        assert_eq!(
            mapper.read(direct),
            expected,
            "unexpected heap direct mapping"
        );
        mapper.write(image, replacement);
        mapper.write(direct, replacement);
    }
    // Valid->invalid or invalid->valid only; publish both writes before reuse.
    crate::memory::sync_translations();
}
