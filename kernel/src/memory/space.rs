//! User address space object payload.
//!
//! The address space never owns physical memory. Every page and page table is
//! referenced through a [`FrameRef`] into the kernel object table; the object
//! table is the single owner. Dropping an `AddressSpace` releases only these
//! references, and the objects become collectable once no capability reaches
//! them.
use super::{
    Error, MAX_PAGES, PAGE_SIZE, USER_END, USER_START, frame::FrameRef, sync_code,
    sync_translations, validate_permissions,
};
use crate::{arch::kernel::vspace::PageTableEntry, config::MemFlags};
use alloc::vec::Vec;
use memory_addr::{PhysAddr, VirtAddr};

struct Page {
    va: usize,
    frame: FrameRef,
    permissions: u64,
    pinned: bool,
}
struct Table {
    index: usize,
    frame: FrameRef,
    managed: bool,
}

/// A validated mapping request. All fallible arithmetic and overlap checks
/// happen in [`AddressSpace::plan_map`]; the object layer allocates the exact
/// frame set and then calls [`AddressSpace::install`]. This keeps allocation
/// and publication separate, so a failed mapping never leaves a partial state.
pub struct MapPlan {
    pub(crate) va: usize,
    pub(crate) permissions: u64,
    pub(crate) pinned: bool,
    pub(crate) tables: Vec<usize>,
    pub(crate) pages: usize,
}

pub struct AddressSpace {
    root: FrameRef,
    l1: FrameRef,
    l2: FrameRef,
    tables: Vec<Table>,
    pages: Vec<Page>,
}

fn store(table: usize, index: usize, entry: PageTableEntry) {
    // SAFETY: private, identity-mapped table; index is a 9-bit table index.
    unsafe {
        (table as *mut PageTableEntry)
            .add(index)
            .write_volatile(entry)
    };
}
fn descriptor(frame: FrameRef, permissions: u64) -> PageTableEntry {
    let mut flags = MemFlags::READ | MemFlags::USER;
    if permissions & 2 != 0 {
        flags |= MemFlags::WRITE;
    }
    if permissions & 4 != 0 {
        flags |= MemFlags::EXECUTE;
    }
    if frame.is_device() {
        // MMIO is Device/NX: never cacheable, never executable.
        flags |= MemFlags::DEVICE;
        flags -= MemFlags::EXECUTE;
    }
    PageTableEntry::new_page(PhysAddr::from_usize(frame.physical()), flags, false)
}

/// Device frames may not be mapped executable.
fn validate_frame(frame: FrameRef, permissions: u64) -> Result<(), Error> {
    if frame.is_device() && permissions & 4 != 0 {
        return Err(Error::InvalidArgument);
    }
    Ok(())
}

impl AddressSpace {
    pub fn new(root: FrameRef, l1: FrameRef, l2: FrameRef) -> Result<Self, Error> {
        // SAFETY: all three frames are uniquely owned by the object table,
        // zeroed, inactive tables installed for the first time.
        unsafe {
            crate::arch::kernel::vspace::paging::prepare_user_tables(
                root.address(),
                l1.address(),
                l2.address(),
            )
        };
        Ok(Self {
            root,
            l1,
            l2,
            tables: Vec::new(),
            pages: Vec::new(),
        })
    }
    pub fn root(&self) -> usize {
        self.root.physical()
    }
    /// Every page-granular object this address space reaches. Used by the
    /// object table to mark mapped frames live during collection.
    pub fn frame_refs(&self) -> impl Iterator<Item = FrameRef> + '_ {
        core::iter::once(self.root)
            .chain(core::iter::once(self.l1))
            .chain(core::iter::once(self.l2))
            .chain(self.tables.iter().map(|table| table.frame))
            .chain(self.pages.iter().map(|page| page.frame))
    }
    fn range(va: usize, len: usize) -> Result<core::ops::Range<usize>, Error> {
        let end = va.checked_add(len).ok_or(Error::InvalidArgument)?;
        if len == 0
            || !va.is_multiple_of(PAGE_SIZE)
            || !len.is_multiple_of(PAGE_SIZE)
            || va < USER_START
            || end > USER_END
        {
            return Err(Error::InvalidArgument);
        }
        Ok(va..end)
    }
    fn index(&self, va: usize) -> Result<usize, Error> {
        self.pages
            .binary_search_by_key(&(va & !(PAGE_SIZE - 1)), |p| p.va)
            .map_err(|_| Error::NotMapped)
    }
    fn table(&self, va: usize) -> usize {
        self.tables
            .iter()
            .find(|t| t.index == va >> 21)
            .unwrap()
            .frame
            .address()
    }

    /// Validate a whole-region mapping and report the L2 tables that must be
    /// created. No memory is allocated and no page table is touched.
    pub fn plan_map(
        &self,
        va: usize,
        len: usize,
        permissions: u64,
        pinned: bool,
    ) -> Result<MapPlan, Error> {
        validate_permissions(permissions)?;
        let range = Self::range(va, len)?;
        let count = len / PAGE_SIZE;
        if count > MAX_PAGES - self.pages.len() {
            return Err(Error::NoMemory);
        }
        for address in range.clone().step_by(PAGE_SIZE) {
            if self.index(address).is_ok() {
                return Err(Error::AlreadyMapped);
            }
        }
        let mut tables = Vec::new();
        tables.try_reserve(64).map_err(|_| Error::NoMemory)?;
        for index in va >> 21..=((range.end - 1) >> 21) {
            if !self.tables.iter().any(|table| table.index == index) {
                tables.push(index);
            }
        }
        Ok(MapPlan {
            va,
            permissions,
            pinned,
            tables,
            pages: count,
        })
    }

    /// Publish a previously planned mapping using frames already owned by the
    /// object table. The frame counts must match the plan exactly.
    pub fn install(
        &mut self,
        plan: MapPlan,
        tables: Vec<FrameRef>,
        pages: Vec<FrameRef>,
    ) -> Result<(), Error> {
        debug_assert_eq!(tables.len(), plan.tables.len());
        debug_assert_eq!(pages.len(), plan.pages);
        self.tables
            .try_reserve(tables.len())
            .map_err(|_| Error::NoMemory)?;
        self.pages
            .try_reserve(pages.len())
            .map_err(|_| Error::NoMemory)?;
        for (index, frame) in plan.tables.iter().copied().zip(tables) {
            store(
                self.l2.address(),
                index,
                PageTableEntry::new_table(PhysAddr::from_usize(frame.physical())),
            );
            self.tables.push(Table {
                index,
                frame,
                managed: true,
            });
        }
        for (offset, frame) in pages.into_iter().enumerate() {
            let va = plan.va + offset * PAGE_SIZE;
            if plan.permissions & 4 != 0 {
                sync_code(frame.address(), PAGE_SIZE);
            }
            store(
                self.table(va),
                (va >> 12) & 511,
                descriptor(frame, plan.permissions),
            );
            self.pages.push(Page {
                va,
                frame,
                permissions: plan.permissions,
                pinned: plan.pinned,
            });
        }
        self.pages.sort_unstable_by_key(|page| page.va);
        sync_translations();
        Ok(())
    }

    /// Query this address space, not the currently installed TTBR0. Includes
    /// the byte offset; mapping existence does not authorize a requested access.
    pub fn translate(&self, va: VirtAddr) -> Result<super::Translation, Error> {
        let va = va.as_usize();
        if !(USER_START..USER_END).contains(&va) {
            return Err(Error::InvalidArgument);
        }
        let page = &self.pages[self.index(va)?];
        // SAFETY: metadata keeps the containing table/frame alive and only this
        // single-core owner can mutate them. The leaf index is bounded to 512.
        let entry = unsafe {
            (self.table(va) as *const PageTableEntry)
                .add((va >> 12) & 511)
                .read_volatile()
        };
        if !entry.is_present() {
            return Err(Error::NotMapped);
        }
        if !entry.is_table_or_page() || entry.physical().as_usize() != page.frame.physical() {
            return Err(Error::InvalidArgument);
        }
        Ok(super::Translation {
            physical: PhysAddr::from_usize(entry.physical().as_usize() + (va & (PAGE_SIZE - 1))),
            flags: entry.flags(),
            page_size: PAGE_SIZE,
        })
    }
    pub fn frame_at(&self, va: usize) -> Result<FrameRef, Error> {
        Ok(self.pages[self.index(va)?].frame)
    }
    /// Install an explicitly supplied small-page object. The containing L3
    /// table must already exist; no implicit object allocation occurs.
    pub fn map_page(&mut self, va: usize, frame: FrameRef, permissions: u64) -> Result<(), Error> {
        Self::range(va, PAGE_SIZE)?;
        validate_permissions(permissions)?;
        validate_frame(frame, permissions)?;
        if self.index(va).is_ok() {
            return Err(Error::AlreadyMapped);
        }
        if self.pages.len() == MAX_PAGES {
            return Err(Error::NoMemory);
        }
        if !self.tables.iter().any(|t| t.index == va >> 21) {
            return Err(Error::NotMapped);
        }
        self.pages.try_reserve(1).map_err(|_| Error::NoMemory)?;
        if permissions & 4 != 0 {
            sync_code(frame.address(), PAGE_SIZE);
        }
        store(
            self.table(va),
            (va >> 12) & 511,
            descriptor(frame, permissions),
        );
        self.pages.push(Page {
            va,
            frame,
            permissions,
            pinned: false,
        });
        self.pages.sort_unstable_by_key(|p| p.va);
        sync_translations();
        Ok(())
    }
    /// This fixed-window VSpace preinstalls L1/L2; object PageTables are L3.
    pub fn map_table(&mut self, va: usize, frame: FrameRef) -> Result<(), Error> {
        if va >= USER_END {
            return Err(Error::InvalidArgument);
        }
        let index = va >> 21;
        if self.tables.iter().any(|t| t.index == index) {
            return Err(Error::AlreadyMapped);
        }
        self.tables.try_reserve(1).map_err(|_| Error::NoMemory)?;
        store(
            self.l2.address(),
            index,
            PageTableEntry::new_table(PhysAddr::from_usize(frame.physical())),
        );
        self.tables.push(Table {
            index,
            frame,
            managed: false,
        });
        sync_translations();
        Ok(())
    }
    pub fn unmap_table(&mut self, va: usize) -> Result<(), Error> {
        let index = va >> 21;
        if self.pages.iter().any(|p| p.va >> 21 == index) {
            return Err(Error::Permission);
        }
        let slot = self
            .tables
            .iter()
            .position(|t| t.index == index)
            .ok_or(Error::NotMapped)?;
        store(self.l2.address(), index, PageTableEntry::empty());
        sync_translations();
        self.tables.remove(slot);
        Ok(())
    }
    fn mutable_range(&self, va: usize, len: usize) -> Result<core::ops::Range<usize>, Error> {
        let range = Self::range(va, len)?;
        for address in range.clone().step_by(PAGE_SIZE) {
            if self.pages[self.index(address)?].pinned {
                return Err(Error::Permission);
            }
        }
        Ok(range)
    }
    pub fn unmap(&mut self, va: usize, len: usize) -> Result<(), Error> {
        let range = self.mutable_range(va, len)?;
        for address in range.clone().step_by(PAGE_SIZE) {
            store(
                self.table(address),
                (address >> 12) & 511,
                PageTableEntry::empty(),
            );
        }
        sync_translations(); // Revoke translations before releasing references.
        self.pages.retain(|page| !range.contains(&page.va));
        for table in &self.tables {
            if table.managed && !self.pages.iter().any(|page| page.va >> 21 == table.index) {
                store(self.l2.address(), table.index, PageTableEntry::empty());
            }
        }
        sync_translations();
        self.tables.retain(|table| {
            !table.managed || self.pages.iter().any(|page| page.va >> 21 == table.index)
        });
        Ok(())
    }
    pub fn protect(&mut self, va: usize, len: usize, permissions: u64) -> Result<(), Error> {
        validate_permissions(permissions)?;
        let range = self.mutable_range(va, len)?;
        for address in range.clone().step_by(PAGE_SIZE) {
            store(
                self.table(address),
                (address >> 12) & 511,
                PageTableEntry::empty(),
            );
        }
        sync_translations(); // Break-before-make for valid descriptor changes.
        for address in range.step_by(PAGE_SIZE) {
            let index = self.index(address)?;
            let table = self.table(address);
            let page = &mut self.pages[index];
            page.permissions = permissions;
            if permissions & 4 != 0 {
                sync_code(page.frame.address(), PAGE_SIZE);
            }
            store(
                table,
                (address >> 12) & 511,
                descriptor(page.frame, permissions),
            );
        }
        sync_translations();
        Ok(())
    }
    pub fn check(&self, va: usize, len: usize, permissions: u64) -> Result<(), Error> {
        let end = va.checked_add(len).ok_or(Error::InvalidArgument)?;
        if va < USER_START || end > USER_END {
            return Err(Error::InvalidArgument);
        }
        if len == 0 {
            return Ok(());
        }
        for address in (va & !(PAGE_SIZE - 1)..end).step_by(PAGE_SIZE) {
            if self.pages[self.index(address)?].permissions & permissions != permissions {
                return Err(Error::Permission);
            }
        }
        Ok(())
    }
    /// Validates the entire user range before copying through owned frame aliases.
    pub fn read(&self, va: usize, buffer: &mut [u8]) -> Result<(), Error> {
        self.check(va, buffer.len(), 1)?;
        let mut offset = 0;
        while offset < buffer.len() {
            let address = va + offset;
            let count = (PAGE_SIZE - (address & (PAGE_SIZE - 1))).min(buffer.len() - offset);
            let mapping = self.translate(VirtAddr::from_usize(address))?;
            let source = super::address::phys_to_virt(mapping.physical)
                .map_err(|_| Error::InvalidArgument)?;
            // SAFETY: complete range validated; mapping owns the source page and
            // the destination is the caller's writable slice. Overlap is allowed.
            unsafe {
                core::ptr::copy(
                    source.as_usize() as *const u8,
                    buffer.as_mut_ptr().add(offset),
                    count,
                );
            }
            offset += count;
        }
        Ok(())
    }
    pub fn write(&mut self, va: usize, buffer: &[u8]) -> Result<(), Error> {
        self.check(va, buffer.len(), 2)?;
        self.initialize(va, buffer)
    }
    /// Loader-only initialization; callers cannot access it through a user syscall.
    pub fn initialize(&mut self, va: usize, buffer: &[u8]) -> Result<(), Error> {
        self.check(va, buffer.len(), 1)?;
        let mut offset = 0;
        while offset < buffer.len() {
            let address = va + offset;
            let count = (PAGE_SIZE - (address & (PAGE_SIZE - 1))).min(buffer.len() - offset);
            let mapping = self.translate(VirtAddr::from_usize(address))?;
            let destination = super::address::phys_to_virt(mapping.physical)
                .map_err(|_| Error::InvalidArgument)?;
            // SAFETY: range validated; initialization uses the owned frame's
            // writable kernel alias while no user task executes on this CPU.
            unsafe {
                core::ptr::copy(
                    buffer.as_ptr().add(offset),
                    destination.as_usize() as *mut u8,
                    count,
                );
            }
            offset += count;
        }
        if !buffer.is_empty() {
            for address in (va & !(PAGE_SIZE - 1)..va + buffer.len()).step_by(PAGE_SIZE) {
                let page = &self.pages[self.index(address)?];
                if page.permissions & 4 != 0 {
                    sync_code(page.frame.address(), PAGE_SIZE);
                }
            }
        }
        Ok(())
    }
}

impl Drop for AddressSpace {
    fn drop(&mut self) {
        use aarch64_cpu::registers::{Readable, TTBR0_EL1};
        // The scheduler must leave this address space before destroying it.
        assert_ne!(TTBR0_EL1.get() as usize, self.root());
        sync_translations();
    }
}
