//! Inactive roots exercise the production mapper without changing CPU mappings.
use crate::{
    arch::kernel::vspace::paging::{MapError, TablePool},
    config::{MemFlags, PAGE_SIZE, RAM_START},
    utils::single_core::SingleCore,
};
use memory_addr::{PhysAddr, VirtAddr};

// Stable storage is required because intermediate descriptors contain real PAs.
static SMALL: SingleCore<TablePool<4>> = SingleCore::new(TablePool::new());
static BOUNDARIES: SingleCore<TablePool<16>> = SingleCore::new(TablePool::new());
const HIGH: usize = 0xffff_0000_0000_0000;

pub fn run() {
    rollback();
    boundaries();
}
fn rollback() {
    let mut pool = SMALL.borrow_mut();
    let root = pool.new_root().unwrap();
    let mut mapper = pool.mapper(root);
    let pa = PhysAddr::from_usize(RAM_START);
    let rw = MemFlags::READ | MemFlags::WRITE;
    let boundary = HIGH + (1 << 21);
    // One root and three subordinate tables fit; a second L3 does not. Failure
    // after the first mapped page must reclaim every new table and leaf.
    assert_eq!(
        mapper.map_region(
            VirtAddr::from_usize(boundary - PAGE_SIZE),
            pa,
            2 * PAGE_SIZE,
            rw
        ),
        Err(MapError::NoTables)
    );
    assert!(matches!(
        mapper.leaf(VirtAddr::from_usize(boundary - PAGE_SIZE)),
        Err(MapError::NotMapped)
    ));

    let va = VirtAddr::from_usize(HIGH + PAGE_SIZE);
    // This succeeds only if the failed map returned the table budget.
    mapper.map_region(va, pa, PAGE_SIZE, rw).unwrap();
    let existing = mapper.leaf(va).unwrap();
    let original = mapper.read(existing);
    // A collision on the second page must roll back the newly written prefix
    // while preserving the existing mapping's PA and access bits.
    assert_eq!(
        mapper.map_region(
            VirtAddr::from_usize(HIGH),
            PhysAddr::from_usize(RAM_START + PAGE_SIZE),
            2 * PAGE_SIZE,
            MemFlags::READ
        ),
        Err(MapError::AlreadyMapped)
    );
    let prefix = mapper.leaf(VirtAddr::from_usize(HIGH)).unwrap();
    assert!(!mapper.read(prefix).is_present());
    assert_eq!(mapper.read(existing), original);
    assert_eq!(original.physical(), pa);

    for (address, physical, size) in [
        (HIGH + 1, RAM_START, PAGE_SIZE),
        (HIGH, RAM_START + 1, PAGE_SIZE),
        (HIGH, RAM_START, 0),
        (HIGH, RAM_START, PAGE_SIZE + 1),
        (0, RAM_START, PAGE_SIZE),
        (usize::MAX - PAGE_SIZE + 1, RAM_START, PAGE_SIZE),
        (HIGH, 1usize << 40, PAGE_SIZE),
    ] {
        assert_eq!(
            mapper.map_region(
                VirtAddr::from_usize(address),
                PhysAddr::from_usize(physical),
                size,
                rw
            ),
            Err(MapError::InvalidRange)
        );
    }
    for flags in [
        MemFlags::WRITE,
        rw | MemFlags::EXECUTE,
        MemFlags::READ | MemFlags::DEVICE | MemFlags::EXECUTE,
        rw | MemFlags::USER,
    ] {
        assert_eq!(
            mapper.map_region(VirtAddr::from_usize(HIGH), pa, PAGE_SIZE, flags),
            Err(MapError::InvalidPermissions)
        );
    }
    assert_eq!(mapper.read(existing), original);
}
fn boundaries() {
    let mut pool = BOUNDARIES.borrow_mut();
    let root = pool.new_root().unwrap();
    let mut mapper = pool.mapper(root);
    // Cross each intermediate-table boundary with physically contiguous pages.
    // Descriptor walking must work for unrelated L0/L1/L2 indices, not arrays
    // tied to the kernel image's particular virtual base.
    for span in [1usize << 21, 1 << 30, 1 << 39] {
        let start = HIGH + span - PAGE_SIZE;
        let pa = RAM_START + PAGE_SIZE;
        mapper
            .map_region(
                VirtAddr::from_usize(start),
                PhysAddr::from_usize(pa),
                2 * PAGE_SIZE,
                MemFlags::READ,
            )
            .unwrap();
        for offset in [0, PAGE_SIZE] {
            let slot = mapper.leaf(VirtAddr::from_usize(start + offset)).unwrap();
            assert_eq!(mapper.read(slot).physical().as_usize(), pa + offset);
        }
    }
}
