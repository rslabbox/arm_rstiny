//! Four-level AArch64 page-table traversal with caller-owned, bounded storage.
//! No kernel layout policy, device addresses, heap allocation or MMU activation.
use super::PageTableEntry;
use crate::config::{MemFlags, PA_MAX_BITS, PAGE_SIZE};
use crate::memory::address::virt_to_phys;
use core::ptr::addr_of;
use memory_addr::{PhysAddr, VirtAddr};

const ENTRIES: usize = 512;
const LEVEL_SHIFTS: [usize; 4] = [39, 30, 21, 12];

#[derive(Clone, Copy)]
#[repr(C, align(4096))]
struct Table([PageTableEntry; ENTRIES]);
impl Table {
    const EMPTY: Self = Self([PageTableEntry::empty(); ENTRIES]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MapError {
    InvalidRange,
    InvalidPermissions,
    AlreadyMapped,
    NotMapped,
    NoTables,
    InvalidTable,
}
#[derive(Clone, Copy)]
pub(crate) struct Root(TableId);
#[derive(Clone, Copy)]
struct TableId(usize);
#[derive(Clone, Copy)]
pub(crate) struct Slot {
    table: TableId,
    index: usize,
}

/// Storage must stay at a fixed kernel-mapped address after the first allocation.
/// Hardware access and activation are the owner's responsibility. Tables are
/// retained for the owner's lifetime, including empty tables used by guard pages.
pub(crate) struct TablePool<const N: usize> {
    tables: [Table; N],
    parents: [Option<Slot>; N],
    used: usize,
}
impl<const N: usize> TablePool<N> {
    pub const fn new() -> Self {
        Self {
            tables: [Table::EMPTY; N],
            parents: [None; N],
            used: 0,
        }
    }
    fn allocate(&mut self, parent: Option<Slot>) -> Result<TableId, MapError> {
        if self.used == N {
            return Err(MapError::NoTables);
        }
        let id = TableId(self.used);
        self.tables[id.0] = Table::EMPTY;
        self.parents[id.0] = parent;
        self.used += 1;
        Ok(id)
    }
    pub fn new_root(&mut self) -> Result<Root, MapError> {
        self.allocate(None).map(Root)
    }
    fn address(&self, id: TableId) -> PhysAddr {
        virt_to_phys(VirtAddr::from_usize(addr_of!(self.tables[id.0]) as usize))
            .expect("page table kernel address")
    }
    pub fn root_address(&self, root: Root) -> PhysAddr {
        self.address(root.0)
    }
    pub fn mapper(&mut self, root: Root) -> Mapper<'_, N> {
        Mapper { pool: self, root }
    }
    fn table_id(&self, entry: PageTableEntry) -> Result<TableId, MapError> {
        if !entry.is_table_or_page() {
            return Err(MapError::InvalidTable);
        }
        let offset = entry
            .physical()
            .as_usize()
            .checked_sub(self.address(TableId(0)).as_usize())
            .ok_or(MapError::InvalidTable)?;
        if offset % PAGE_SIZE != 0 || offset / PAGE_SIZE >= self.used {
            return Err(MapError::InvalidTable);
        }
        Ok(TableId(offset / PAGE_SIZE))
    }
    fn read(&self, slot: Slot) -> PageTableEntry {
        self.tables[slot.table.0].0[slot.index]
    }
    fn write(&mut self, slot: Slot, entry: PageTableEntry) {
        self.tables[slot.table.0].0[slot.index] = entry;
    }
    fn rollback_tables(&mut self, checkpoint: usize) {
        for index in (checkpoint..self.used).rev() {
            if let Some(parent) = self.parents[index] {
                self.write(parent, PageTableEntry::empty());
            }
        }
        self.used = checkpoint;
    }
}

pub(crate) struct Mapper<'a, const N: usize> {
    pool: &'a mut TablePool<N>,
    root: Root,
}
impl<const N: usize> Mapper<'_, N> {
    fn walk(&mut self, va: VirtAddr, allocate: bool) -> Result<Slot, MapError> {
        let mut table = self.root.0;
        for shift in &LEVEL_SHIFTS[..3] {
            let slot = Slot {
                table,
                index: (va.as_usize() >> shift) & (ENTRIES - 1),
            };
            let entry = self.pool.read(slot);
            table = if entry.is_present() {
                self.pool.table_id(entry)?
            } else if allocate {
                let next = self.pool.allocate(Some(slot))?;
                self.pool
                    .write(slot, PageTableEntry::new_table(self.pool.address(next)));
                next
            } else {
                return Err(MapError::NotMapped);
            };
        }
        Ok(Slot {
            table,
            index: (va.as_usize() >> LEVEL_SHIFTS[3]) & (ENTRIES - 1),
        })
    }
    /// Locate an existing L3 slot, including an invalid guard-page entry.
    pub fn leaf(&mut self, va: VirtAddr) -> Result<Slot, MapError> {
        validate_virtual(va.as_usize(), PAGE_SIZE)?;
        self.walk(va, false)
    }
    /// Resolve an arbitrary byte address from actual leaf descriptors.
    pub fn translate(&mut self, va: VirtAddr) -> Result<crate::memory::Translation, MapError> {
        let page = VirtAddr::from_usize(va.as_usize() & !(PAGE_SIZE - 1));
        let slot = self.leaf(page)?;
        let entry = self.read(slot);
        if !entry.is_present() {
            return Err(MapError::NotMapped);
        }
        if !entry.is_table_or_page() {
            return Err(MapError::InvalidTable);
        }
        Ok(crate::memory::Translation {
            physical: PhysAddr::from_usize(
                entry.physical().as_usize() + (va.as_usize() & (PAGE_SIZE - 1)),
            ),
            flags: entry.flags(),
            page_size: PAGE_SIZE,
        })
    }
    pub fn read(&self, slot: Slot) -> PageTableEntry {
        self.pool.read(slot)
    }
    pub fn write(&mut self, slot: Slot, entry: PageTableEntry) {
        self.pool.write(slot, entry);
    }

    /// Map a linear, page-aligned region into an inactive root. Failure restores
    /// both previous mappings and pool capacity. Never silently overwrite a PTE.
    pub fn map_region(
        &mut self,
        va: VirtAddr,
        pa: PhysAddr,
        size: usize,
        flags: MemFlags,
    ) -> Result<(), MapError> {
        validate_virtual(va.as_usize(), size)?;
        if pa.as_usize() % PAGE_SIZE != 0
            || pa
                .as_usize()
                .checked_add(size)
                .is_none_or(|end| end > 1usize << PA_MAX_BITS)
        {
            return Err(MapError::InvalidRange);
        }
        if !flags.contains(MemFlags::READ)
            || flags.contains(MemFlags::WRITE | MemFlags::EXECUTE)
            || flags.contains(MemFlags::DEVICE | MemFlags::EXECUTE)
            || flags.contains(MemFlags::USER)
        {
            return Err(MapError::InvalidPermissions);
        }
        let checkpoint = self.pool.used;
        let mut mapped = 0;
        let result = (|| {
            while mapped < size {
                let slot = self.walk(VirtAddr::from_usize(va.as_usize() + mapped), true)?;
                if self.read(slot).is_present() {
                    return Err(MapError::AlreadyMapped);
                }
                self.write(
                    slot,
                    PageTableEntry::new_page(
                        PhysAddr::from_usize(pa.as_usize() + mapped),
                        flags,
                        false,
                    ),
                );
                mapped += PAGE_SIZE;
            }
            Ok(())
        })();
        if result.is_err() {
            for offset in (0..mapped).step_by(PAGE_SIZE) {
                let slot = self
                    .walk(VirtAddr::from_usize(va.as_usize() + offset), false)
                    .expect("staged mapping");
                self.write(slot, PageTableEntry::empty());
            }
            self.pool.rollback_tables(checkpoint);
        }
        result
    }
}

fn validate_virtual(start: usize, size: usize) -> Result<(), MapError> {
    // Kernel TTBR1 uses the upper 48-bit window; no low aliases are accepted.
    if start < 0xffff_0000_0000_0000
        || start % PAGE_SIZE != 0
        || size == 0
        || size % PAGE_SIZE != 0
        || start.checked_add(size).is_none()
    {
        Err(MapError::InvalidRange)
    } else {
        Ok(())
    }
}

/// Install the initial low-address hierarchy for the supported user VA window.
/// # Safety
/// All frames must be distinct, zeroed, aligned, writable and inactive. Addresses
/// must belong to the kernel image or physical direct map.
pub(crate) unsafe fn prepare_user_tables(root: usize, l1: usize, l2: usize) {
    for address in [root, l1, l2] {
        assert!(address.is_multiple_of(PAGE_SIZE));
    }
    assert!(root != l1 && root != l2 && l1 != l2);
    // SAFETY: only the first entry of each caller-owned inactive table is changed.
    unsafe {
        (root as *mut PageTableEntry).write(PageTableEntry::new_table(
            virt_to_phys(VirtAddr::from_usize(l1)).expect("user table alias"),
        ));
        (l1 as *mut PageTableEntry).write(PageTableEntry::new_table(
            virt_to_phys(VirtAddr::from_usize(l2)).expect("user table alias"),
        ));
    }
}
