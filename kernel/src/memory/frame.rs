//! Unique frames from the reserved pool and elfloader's loaded image.
use super::{Error, PAGE_SIZE};
use crate::memory::address::{phys_to_virt, virt_to_phys};
use crate::object::ObjectId;
use crate::utils::single_core::SingleCore;
use core::ptr::addr_of;

/// A non-owning reference to a page-granular kernel object (frame or page table).
///
/// The object table is the single owner of the underlying [`Frame`]; this value
/// only carries the object identity plus the immutable addresses needed to
/// install page-table entries. Dropping a `FrameRef` never frees memory; the
/// object becomes collectable when no capability or live address space reaches
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameRef {
    id: ObjectId,
    virt: usize,
    physical: usize,
    device: bool,
}
impl FrameRef {
    pub const fn new(id: ObjectId, virt: usize, physical: usize, device: bool) -> Self {
        Self {
            id,
            virt,
            physical,
            device,
        }
    }
    pub const fn id(self) -> ObjectId {
        self.id
    }
    /// Kernel direct-map alias used for software access and cache maintenance.
    pub const fn address(self) -> usize {
        self.virt
    }
    /// Physical base address installed into page-table descriptors.
    pub const fn physical(self) -> usize {
        self.physical
    }
    /// Device MMIO frames must use the Device memory attribute and stay
    /// non-executable.
    pub const fn is_device(self) -> bool {
        self.device
    }
}
const FRAME_COUNT: usize = 2048;
const BOOT_FRAMES: usize = kernel_abi::MAX_USER_PAGES;
struct Pools {
    primary: [u64; FRAME_COUNT / 64],
    boot: [u64; BOOT_FRAMES / 64],
    boot_start: usize,
    boot_pages: usize,
    boot_ready: bool,
    /// The boot-module archive range: published to the root task as Frame
    /// capabilities over pages the bootloader copied into free RAM.
    modules: [u64; BOOT_FRAMES / 64],
    modules_start: usize,
    modules_pages: usize,
}
static POOL: SingleCore<Pools> = SingleCore::new(Pools {
    primary: [0; FRAME_COUNT / 64],
    boot: [u64::MAX; BOOT_FRAMES / 64],
    boot_start: 0,
    boot_pages: 0,
    boot_ready: false,
    modules: [u64::MAX; BOOT_FRAMES / 64],
    modules_start: 0,
    modules_pages: 0,
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
/// Adopt the boot-module archive range so `take_boot` can hand its pages out
/// as Frame objects. Optional: an absent archive leaves the range empty.
pub fn prepare_modules(start: usize, end: usize) {
    if end == start {
        return;
    }
    assert!(start.is_multiple_of(PAGE_SIZE) && end.is_multiple_of(PAGE_SIZE));
    assert!(end > start && (end - start) / PAGE_SIZE <= BOOT_FRAMES);
    // SAFETY: one-shot boot initialization with IRQs masked.
    let mut guard = POOL.borrow_mut();
    let pools = &mut *guard;
    assert_eq!(pools.modules_start, 0);
    pools.modules_start = phys_to_virt(memory_addr::PhysAddr::from_usize(start))
        .expect("direct-map address")
        .as_usize();
    pools.modules_pages = (end - start) / PAGE_SIZE;
    for index in 0..pools.modules_pages {
        pools.modules[index / 64] &= !(1 << (index % 64));
    }
}

pub fn finish_boot() {
    // SAFETY: all loaded pages have been claimed before holes become allocatable.
    POOL.borrow_mut().boot_ready = true;
}

/// Where a frame's physical page is accounted. Boot objects come from the
/// kernel frame pool; user objects come from an Untyped region, whose `reset`
/// (not `Drop`) reclaims the memory.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FrameOrigin {
    Pool,
    Untyped,
}

pub struct Frame {
    address: usize,
    origin: FrameOrigin,
    device: bool,
}

impl Frame {
    /// Carve a frame from the kernel boot pool. Only boot objects and kernel
    /// self-tests may use this; user frames come from Untyped retype.
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
                    let address = start + (word_index * 64 + bit) * PAGE_SIZE;
                    // SAFETY: exclusively owned frame in the kernel's high mapping.
                    unsafe { core::ptr::write_bytes(address as *mut u8, 0, PAGE_SIZE) };
                    return Ok(Self {
                        address,
                        origin: FrameOrigin::Pool,
                        device: false,
                    });
                }
            }
        }
        Err(Error::NoMemory)
    }
    /// Adopt a physical page already carved from an Untyped region. The frame
    /// does not own the page: only `Untyped::reset` reclaims it.
    pub fn from_untyped(physical: usize, is_device: bool) -> Result<Self, Error> {
        if !physical.is_multiple_of(PAGE_SIZE) {
            return Err(Error::InvalidArgument);
        }
        let address = phys_to_virt(memory_addr::PhysAddr::from_usize(physical))
            .map_err(|_| Error::InvalidArgument)?
            .as_usize();
        Ok(Self {
            address,
            origin: FrameOrigin::Untyped,
            device: is_device,
        })
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
        if !physical.is_multiple_of(PAGE_SIZE) {
            return Err(Error::InvalidArgument);
        }
        // Two adopted ranges: the loaded root image (claimable only while boot
        // allocation is disabled) and the fully pre-claimed module archive.
        let in_boot = pools.boot_start != 0
            && address >= pools.boot_start
            && address < pools.boot_start + pools.boot_pages * PAGE_SIZE;
        let in_modules = pools.modules_start != 0
            && address >= pools.modules_start
            && address < pools.modules_start + pools.modules_pages * PAGE_SIZE;
        let (start, pages, bits) = if in_boot && !pools.boot_ready {
            (pools.boot_start, pools.boot_pages, &mut pools.boot[..])
        } else if in_modules {
            (
                pools.modules_start,
                pools.modules_pages,
                &mut pools.modules[..],
            )
        } else {
            return Err(Error::InvalidArgument);
        };
        let index = (address - start) / PAGE_SIZE;
        let mask = 1 << (index % 64);
        if index >= pages || bits[index / 64] & mask != 0 {
            return Err(Error::AlreadyMapped);
        }
        bits[index / 64] |= mask;
        Ok(Self {
            address,
            origin: FrameOrigin::Pool,
            device: false,
        })
    }
    pub fn address(&self) -> usize {
        self.address
    }
    pub fn physical(&self) -> usize {
        virt_to_phys(memory_addr::VirtAddr::from_usize(self.address))
            .expect("kernel address")
            .as_usize()
    }
    pub const fn is_device(&self) -> bool {
        self.device
    }
}
impl Drop for Frame {
    fn drop(&mut self) {
        debug_assert!(crate::arch::machine::instructions::irq_masked());
        if self.origin == FrameOrigin::Untyped {
            // The Untyped region owns the page; `reset` reclaims it in bulk.
            return;
        }
        // SAFETY: mappings are revoked before the unique frame owner is released.
        let mut guard = POOL.borrow_mut();
        let pools = &mut *guard;
        let (start, bits) = if pools.boot_start != 0 && self.address >= pools.boot_start {
            (pools.boot_start, &mut pools.boot[..])
        } else if pools.modules_start != 0 && self.address >= pools.modules_start {
            (pools.modules_start, &mut pools.modules[..])
        } else {
            (primary_start(), &mut pools.primary[..])
        };
        let index = (self.address - start) / PAGE_SIZE;
        assert_ne!(bits[index / 64] & (1 << (index % 64)), 0);
        bits[index / 64] &= !(1 << (index % 64));
    }
}
#[cfg_attr(not(feature = "kernel-test"), allow(dead_code))]
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
