//! Untyped physical memory: a real region with a monotonic watermark.
//!
//! The object table owns the payload; user capabilities own the *right* to
//! retype it. `allocate` only advances the watermark, so a single object cannot
//! be freed in isolation. Reclamation is region-granular: `Revoke` finalises
//! every child object and resets the watermark, which is what makes teardown
//! deterministic.
use crate::memory::PAGE_SIZE;

/// Maximum region size is `2^30` bytes; the platform direct map covers 40 bits.
pub const MAX_SIZE_BITS: u8 = 30;
pub const MIN_SIZE_BITS: u8 = 12;

pub struct Untyped {
    physical: usize,
    size_bits: u8,
    is_device: bool,
    free_offset: usize,
}
impl Untyped {
    /// The caller guarantees `physical` is aligned to `2^size_bits` and the
    /// region is disjoint from every other live region.
    pub fn new(physical: usize, size_bits: u8, is_device: bool) -> Self {
        assert!((MIN_SIZE_BITS..=MAX_SIZE_BITS).contains(&size_bits));
        assert!(physical.is_multiple_of(1usize << size_bits));
        Self {
            physical,
            size_bits,
            is_device,
            free_offset: 0,
        }
    }
    pub const fn physical(&self) -> usize {
        self.physical
    }
    #[allow(dead_code)]
    pub const fn size_bits(&self) -> u8 {
        self.size_bits
    }
    pub const fn is_device(&self) -> bool {
        self.is_device
    }
    pub const fn size(&self) -> usize {
        1usize << self.size_bits
    }
    #[allow(dead_code)]
    pub const fn free_offset(&self) -> usize {
        self.free_offset
    }
    pub const fn remaining(&self) -> usize {
        self.size() - self.free_offset
    }
    /// Carve `size` bytes with `align`-byte alignment from the watermark.
    /// Returns the physical base and advances the watermark; a failed request
    /// leaves the watermark unchanged.
    pub fn allocate(&mut self, size: usize, align: usize) -> Option<usize> {
        if size == 0 || !align.is_power_of_two() {
            return None;
        }
        let base = self.physical;
        let start = base.checked_add(self.free_offset)?.checked_add(align - 1)? & !(align - 1);
        let end = start.checked_add(size)?;
        if end > base.checked_add(self.size())? {
            return None;
        }
        self.free_offset = end - base;
        Some(start)
    }
    /// Discard every allocation. The caller must have finalised all children
    /// first; this only rewinds the watermark.
    pub fn reset(&mut self) {
        self.free_offset = 0;
    }
    /// Rewind to a previously saved watermark after a failed multi-object
    /// allocation. The caller must have removed the objects created since then.
    pub fn reset_to(&mut self, offset: usize) {
        debug_assert!(offset <= self.free_offset);
        self.free_offset = offset;
    }
    /// Whether `count` aligned `(size, align)` allocations fit from the current
    /// watermark without mutating it.
    pub fn fits(&self, count: usize, size: usize, align: usize) -> bool {
        if size == 0 || !align.is_power_of_two() {
            return false;
        }
        let base = self.physical;
        let limit = match base.checked_add(self.size()) {
            Some(limit) => limit,
            None => return false,
        };
        let mut offset = self.free_offset;
        for _ in 0..count {
            let Some(aligned) = base
                .checked_add(offset)
                .and_then(|v| v.checked_add(align - 1))
            else {
                return false;
            };
            let start = aligned & !(align - 1);
            let Some(end) = start.checked_add(size) else {
                return false;
            };
            if end > limit {
                return false;
            }
            offset = end - base;
        }
        true
    }
    /// Zero the region for ordinary memory. Device memory keeps its contents.
    pub fn clear(&self) {
        if self.is_device {
            return;
        }
        zero(self.physical, self.size());
    }
}

/// Zero a physical extent through the kernel direct map.
pub fn zero(physical: usize, size: usize) {
    if let Ok(address) =
        crate::memory::address::phys_to_virt(memory_addr::PhysAddr::from_usize(physical))
    {
        // SAFETY: the caller exclusively owns the region and the direct map is
        // writable for all RAM above the firmware window.
        unsafe {
            core::ptr::write_bytes(address.as_usize() as *mut u8, 0, size);
        }
    }
}

/// Split a free physical region into aligned power-of-two Untyped blocks.
/// Emits `(physical, size_bits)` in ascending address order.
pub fn partition(mut start: usize, end: usize, mut emit: impl FnMut(usize, u8)) {
    start = start.next_multiple_of(PAGE_SIZE);
    while start < end {
        let remaining = end - start;
        let by_size = remaining.next_power_of_two() >> 1;
        let by_alignment = if start == 0 {
            by_size
        } else {
            1usize << start.trailing_zeros()
        };
        let size = by_size.min(by_alignment).max(PAGE_SIZE);
        let size_bits = size.trailing_zeros() as u8;
        emit(start, size_bits.clamp(MIN_SIZE_BITS, MAX_SIZE_BITS));
        start += size;
    }
}
