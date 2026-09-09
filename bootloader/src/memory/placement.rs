//! Allocation-free search around firmware and loader reservations.
use super::{Error, Region};

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
