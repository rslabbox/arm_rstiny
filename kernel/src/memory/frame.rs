//! Unique frames from the reserved pool and elfloader's loaded image.
use super::{Error, PAGE_SIZE};
use crate::memory::address::{phys_to_virt, virt_to_phys};
use crate::utils::single_core::SingleCore;
use core::ptr::addr_of;
const FRAME_COUNT: usize = 2048;
const BOOT_FRAMES: usize = kernel_abi::MAX_USER_PAGES;
struct Pools {
    primary: [u64; FRAME_COUNT / 64],
    boot: [u64; BOOT_FRAMES / 64],
    boot_start: usize,
    boot_pages: usize,
    boot_ready: bool,
}
static POOL: SingleCore<Pools> = SingleCore::new(Pools {
    primary: [0; FRAME_COUNT / 64],
    boot: [u64::MAX; BOOT_FRAMES / 64],
    boot_start: 0,
    boot_pages: 0,
    boot_ready: false,
});
unsafe extern "C" {
    static __frames_start: u8;
    static __frames_end: u8;
}

fn primary_start() -> usize {
    let physical = virt_to_phys(memory_addr::VirtAddr::from_usize(
        addr_of!(__frames_start) as usize
    ))
    .expect("frame pool image address");
    phys_to_virt(physical)
        .expect("frame pool direct alias")
        .as_usize()
}

pub fn prepare_boot(start: usize, end: usize) {
    assert!(start.is_multiple_of(PAGE_SIZE) && end.is_multiple_of(PAGE_SIZE));
    assert!(end > start && (end - start) / PAGE_SIZE <= BOOT_FRAMES);
    assert!(
        start
            >= super::address::kernel_image()
                .expect("kernel image")
                .physical_end()
                .as_usize()
    );
    // SAFETY: one-shot boot initialization with IRQs masked.
    let mut guard = POOL.borrow_mut();
    let pools = &mut *guard;
    assert_eq!(pools.boot_start, 0);
    pools.boot_start = phys_to_virt(memory_addr::PhysAddr::from_usize(start))
        .expect("direct-map address")
        .as_usize();
    pools.boot_pages = (end - start) / PAGE_SIZE;
    for index in 0..pools.boot_pages {
        pools.boot[index / 64] &= !(1 << (index % 64));
    }
}
pub fn finish_boot() {
    // SAFETY: all loaded pages have been claimed before holes become allocatable.
    POOL.borrow_mut().boot_ready = true;
}
pub struct Frame(usize);
impl Frame {
    pub fn allocate() -> Result<Self, Error> {
        debug_assert!(crate::arch::machine::instructions::irq_masked());
        assert_eq!(
            addr_of!(__frames_end) as usize - addr_of!(__frames_start) as usize,
            FRAME_COUNT * PAGE_SIZE
        );
        // SAFETY: exclusive allocator access with IRQs masked.
        let mut guard = POOL.borrow_mut();
        let pools = &mut *guard;
        let boot_start = pools.boot_start;
        let boot_ready = pools.boot_ready;
        for (start, bits) in [
            (primary_start(), &mut pools.primary[..]),
            (boot_start, &mut pools.boot[..]),
        ] {
            if start == boot_start && !boot_ready {
                continue;
            }
            for (word_index, word) in bits.iter_mut().enumerate() {
                if *word != u64::MAX {
                    let bit = (!*word).trailing_zeros() as usize;
                    *word |= 1 << bit;
                    let frame = Self(start + (word_index * 64 + bit) * PAGE_SIZE);
                    // SAFETY: exclusively owned frame in the kernel's high mapping.
                    unsafe { core::ptr::write_bytes(frame.0 as *mut u8, 0, PAGE_SIZE) };
                    return Ok(frame);
                }
            }
        }
        Err(Error::NoMemory)
    }
    pub fn take_boot(physical: usize) -> Result<Self, Error> {
        let address = phys_to_virt(memory_addr::PhysAddr::from_usize(physical))
            .map_err(|_| Error::InvalidArgument)?
            .as_usize();
        let mapped = super::kernel::translate(memory_addr::VirtAddr::from_usize(address))
            .map_err(|_| Error::NotMapped)?;
        if mapped.physical.as_usize() != physical {
            return Err(Error::InvalidArgument);
        }
        // SAFETY: one-shot ownership transfer before boot allocation is enabled.
        let mut guard = POOL.borrow_mut();
        let pools = &mut *guard;
        if pools.boot_ready
            || !physical.is_multiple_of(PAGE_SIZE)
            || address < pools.boot_start
            || address >= pools.boot_start + pools.boot_pages * PAGE_SIZE
        {
            return Err(Error::InvalidArgument);
        }
        let index = (address - pools.boot_start) / PAGE_SIZE;
        let mask = 1 << (index % 64);
        if pools.boot[index / 64] & mask != 0 {
            return Err(Error::AlreadyMapped);
        }
        pools.boot[index / 64] |= mask;
        Ok(Self(address))
    }
    pub fn address(&self) -> usize {
        self.0
    }
    pub fn physical(&self) -> usize {
        virt_to_phys(memory_addr::VirtAddr::from_usize(self.0))
            .expect("kernel address")
            .as_usize()
    }
}
impl Drop for Frame {
    fn drop(&mut self) {
        debug_assert!(crate::arch::machine::instructions::irq_masked());
        // SAFETY: mappings are revoked before the unique frame owner is released.
        let mut guard = POOL.borrow_mut();
        let pools = &mut *guard;
        let (start, bits) = if pools.boot_start != 0 && self.0 >= pools.boot_start {
            (pools.boot_start, &mut pools.boot[..])
        } else {
            (primary_start(), &mut pools.primary[..])
        };
        let index = (self.0 - start) / PAGE_SIZE;
        assert_ne!(bits[index / 64] & (1 << (index % 64)), 0);
        bits[index / 64] &= !(1 << (index % 64));
    }
}
pub fn available() -> usize {
    // SAFETY: same IRQ-masked ownership domain as allocation.
    let pools = POOL.borrow_mut();
    pools
        .primary
        .iter()
        .map(|word| word.count_zeros() as usize)
        .sum::<usize>()
        + if pools.boot_ready {
            pools
                .boot
                .iter()
                .map(|word| word.count_zeros() as usize)
                .sum()
        } else {
            0
        }
}
