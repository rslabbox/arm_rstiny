//! Address ranges with checked extents.
use super::Error;
use memory_addr::{PhysAddr, VirtAddr};

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
    pub(super) fn overlaps(self, other: Self) -> bool {
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
