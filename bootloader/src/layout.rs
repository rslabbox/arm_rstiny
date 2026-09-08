//! Checked, allocation-free placement into RAM excluding reserved ranges.
use memory_addr::{PhysAddr, VirtAddr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Overflow,
    InvalidRange,
    NoMemory,
    KernelWindow,
    RootLayout,
    HeaderPage,
    LoadMinimum,
}

#[derive(Clone, Copy, Debug)]
pub struct Region {
    start: PhysAddr,
    end: PhysAddr,
}
impl Region {
    pub fn new(start: usize, size: usize) -> Result<Self, Error> {
        let end = start.checked_add(size).ok_or(Error::Overflow)?;
        if size == 0 {
            return Err(Error::InvalidRange);
        }
        Ok(Self {
            start: PhysAddr::from_usize(start),
            end: PhysAddr::from_usize(end),
        })
    }
    pub fn start(self) -> usize {
        self.start.as_usize()
    }
    pub fn end(self) -> usize {
        self.end.as_usize()
    }
    pub fn size(self) -> usize {
        self.end() - self.start()
    }
    fn overlaps(self, other: Self) -> bool {
        self.start() < other.end() && other.start() < self.end()
    }
}
#[derive(Clone, Copy, Debug)]
pub struct ImageMapping {
    physical: Region,
    virtual_start: VirtAddr,
}
impl ImageMapping {
    pub fn new(physical: Region, virtual_start: usize) -> Result<Self, Error> {
        virtual_start
            .checked_add(physical.size())
            .ok_or(Error::Overflow)?;
        Ok(Self {
            physical,
            virtual_start: VirtAddr::from_usize(virtual_start),
        })
    }
    pub fn physical(self) -> Region {
        self.physical
    }
    pub fn virtual_start(self) -> usize {
        self.virtual_start.as_usize()
    }
}
/// First aligned gap large enough for the complete boot image set. Reserved
/// ranges may be unsorted and overlap; no memory is written during this search.
pub fn allocate(
    ram: Region,
    reserved: &[Region],
    minimum: usize,
    size: usize,
    alignment: usize,
) -> Result<Region, Error> {
    if size == 0 || !alignment.is_power_of_two() {
        return Err(Error::InvalidRange);
    }
    let mut start = minimum.max(ram.start());
    loop {
        start = start.checked_add(alignment - 1).ok_or(Error::Overflow)? & !(alignment - 1);
        let candidate = Region::new(start, size)?;
        if candidate.end() > ram.end() {
            return Err(Error::NoMemory);
        }
        match reserved
            .iter()
            .filter(|r| candidate.overlaps(**r))
            .map(|r| r.end())
            .max()
        {
            Some(end) => start = end,
            None => return Ok(candidate),
        }
    }
}
