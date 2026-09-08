use super::{
    Error, MAX_PAGES, PAGE_SIZE, USER_END, USER_START, frame::Frame, sync_code, sync_translations,
    validate_permissions,
};
use crate::{arch::PageTableEntry, config::MemFlags};
use alloc::vec::Vec;
use memory_addr::PhysAddr;

struct Page {
    va: usize,
    frame: Frame,
    permissions: u64,
    pinned: bool,
}
struct Table {
    index: usize,
    frame: Frame,
}

pub struct AddressSpace {
    root: Frame,
    _l1: Frame,
    l2: Frame,
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
fn descriptor(frame: &Frame, permissions: u64) -> PageTableEntry {
    let mut flags = MemFlags::READ | MemFlags::USER;
    if permissions & 2 != 0 {
        flags |= MemFlags::WRITE;
    }
    if permissions & 4 != 0 {
        flags |= MemFlags::EXECUTE;
    }
    PageTableEntry::new_page(PhysAddr::from_usize(frame.physical()), flags, false)
}

impl AddressSpace {
    pub fn new() -> Result<Self, Error> {
        let root = Frame::allocate()?;
        let l1 = Frame::allocate()?;
        let l2 = Frame::allocate()?;
        // SAFETY: all three frames are uniquely owned, zeroed, inactive tables.
        unsafe {
            crate::arch::boot::prepare_user_tables(root.address(), l1.address(), l2.address())
        };
        Ok(Self {
            root,
            _l1: l1,
            l2,
            tables: Vec::new(),
            pages: Vec::new(),
        })
    }
    pub fn root(&self) -> usize {
        self.root.physical()
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

    /// Stage all fallible allocations before publishing any mapping.
    pub fn map(
        &mut self,
        va: usize,
        len: usize,
        permissions: u64,
        pinned: bool,
    ) -> Result<(), Error> {
        self.map_frames(va, len, permissions, pinned, None)
    }

    pub fn map_loaded(
        &mut self,
        va: usize,
        physical: usize,
        len: usize,
        permissions: u64,
    ) -> Result<(), Error> {
        self.map_frames(va, len, permissions, false, Some(physical))
    }

    fn map_frames(
        &mut self,
        va: usize,
        len: usize,
        permissions: u64,
        pinned: bool,
        loaded: Option<usize>,
    ) -> Result<(), Error> {
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
        let mut pages = Vec::new();
        let mut tables = Vec::new();
        pages
            .try_reserve_exact(count)
            .map_err(|_| Error::NoMemory)?;
        tables
            .try_reserve_exact(((range.end - 1) >> 21) - (va >> 21) + 1)
            .map_err(|_| Error::NoMemory)?;
        self.pages.try_reserve(count).map_err(|_| Error::NoMemory)?;
        for index in va >> 21..=((range.end - 1) >> 21) {
            if !self.tables.iter().any(|t| t.index == index) {
                tables.push(Table {
                    index,
                    frame: Frame::allocate()?,
                });
            }
        }
        self.tables
            .try_reserve(tables.len())
            .map_err(|_| Error::NoMemory)?;
        for address in range.step_by(PAGE_SIZE) {
            pages.push(Page {
                va: address,
                frame: match loaded {
                    Some(physical) => Frame::take_boot(physical + address - va)?,
                    None => Frame::allocate()?,
                },
                permissions,
                pinned,
            });
        }
        for table in tables {
            store(
                self.l2.address(),
                table.index,
                PageTableEntry::new_table(PhysAddr::from_usize(table.frame.physical())),
            );
            self.tables.push(table);
        }
        for page in pages {
            if permissions & 4 != 0 {
                sync_code(page.frame.address(), PAGE_SIZE);
            }
            store(
                self.table(page.va),
                (page.va >> 12) & 511,
                descriptor(&page.frame, permissions),
            );
            self.pages.push(page);
        }
        self.pages.sort_unstable_by_key(|page| page.va);
        sync_translations();
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
        sync_translations(); // Revoke translations before releasing physical ownership.
        self.pages.retain(|page| !range.contains(&page.va));
        for table in &self.tables {
            if !self.pages.iter().any(|page| page.va >> 21 == table.index) {
                store(self.l2.address(), table.index, PageTableEntry::empty());
            }
        }
        sync_translations();
        self.tables
            .retain(|table| self.pages.iter().any(|page| page.va >> 21 == table.index));
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
                descriptor(&page.frame, permissions),
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
        for (offset, byte) in buffer.iter_mut().enumerate() {
            let page = &self.pages[self.index(va + offset)?];
            // SAFETY: range validated and the frame remains owned throughout copying.
            *byte = unsafe {
                *((page.frame.address() + ((va + offset) & (PAGE_SIZE - 1))) as *const u8)
            };
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
        for (offset, byte) in buffer.iter().enumerate() {
            let page = &self.pages[self.index(va + offset)?];
            // SAFETY: mapping is owned and no user runs concurrently on this CPU.
            unsafe {
                *((page.frame.address() + ((va + offset) & (PAGE_SIZE - 1))) as *mut u8) = *byte
            };
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
