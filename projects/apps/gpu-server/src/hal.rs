//! The GPU service's DMA allocator and virtio Hal: frames are retyped from
//! the service budget into fixed CSpace slots and mapped into the one 2 MiB
//! window (docs/gui-display.md §10).

use core::ptr::NonNull;

use rstiny::capability::{
    CNode, CPtr, INIT_CNODE, INIT_UNTYPED, INIT_VSPACE, ObjectType, Page, RIGHTS_READ,
    RIGHTS_WRITE, Untyped, VM_CACHEABLE, VM_EXECUTE_NEVER,
};
use virtio_drivers::{BufferDirection, Hal, PhysAddr};

// One 2 MiB window covers the MMIO mapping, the DMA frames (queues +
// framebuffer) and nothing else, so a single L3 table from the service
// budget backs all of it (docs/gui-display.md §10: budget = 2M).
pub const WINDOW_VA: usize = 0x0400_0000;
pub const MMIO_PAGES: u64 = 4; // 16 KiB VirtIO MMIO window from the device Untyped
pub const MMIO_VA: usize = WINDOW_VA;
pub const DMA_VA: usize = WINDOW_VA + 0x8000; // DMA frames handed out by Hal
pub const DMA_MAX_PAGES: usize = 512; // queue rings + 300 framebuffer pages
pub const MMIO_PAGE: u64 = 40;
pub const TABLE_SLOT: u64 = 44;
/// DMA frames, handed out sequentially. The window starts *above* the fixed
/// child CSpace layout (control 140, console 51, self 52, deps 53.., IRQ
/// 56..): a 300-page framebuffer spans 300 slots and must not run into them.
pub const DMA_SLOT: u64 = 200;
pub const NT_GPU: u64 = DMA_SLOT + DMA_MAX_PAGES as u64; // flush completion
pub const NT_GPU_BADGED: u64 = NT_GPU + 1;
pub const NT_INPUT: u64 = NT_GPU + 2; // keyboard event line
pub const NT_INPUT_BADGED: u64 = NT_INPUT + 1;
/// Landing slots for the RELEASE batches. The server keeps its master caps
/// in fb_slot.. (crash recovery), so returned caps must land elsewhere; each
/// verified batch is deleted again, keeping the cap count flat.
pub const REL_SLOT: u64 = NT_INPUT_BADGED + 1;
/// Delivery badges minted onto the bound Notification copies. Non-zero so an
/// unwaited signal merges into the notification bits instead of being lost.
pub const IRQ_BADGE_GPU: u64 = 1;
pub const IRQ_BADGE_INPUT: u64 = 2;


/// The framebuffer's place in the service's CSpace: the Hal records every
/// allocation, and the one as large as the framebuffer *is* the framebuffer
/// (`setup_framebuffer` allocates exactly `width * height * 4` bytes after
/// the queue rings). Those are the caps LEASE hands to the client.
#[derive(Clone, Copy)]
struct Allocation {
    first_slot: u64,
    pages: usize,
    vaddr: usize,
}
static mut ALLOCATIONS: [Allocation; 16] = [Allocation {
    first_slot: 0,
    pages: 0,
    vaddr: 0,
}; 16];
static mut ALLOCATIONS_USED: usize = 0;

fn record_allocation(first_slot: u64, pages: usize, vaddr: usize) {
    // SAFETY: single-core user task; DMA setup never re-enters.
    unsafe {
        let used = core::ptr::addr_of_mut!(ALLOCATIONS_USED);
        let log = core::ptr::addr_of_mut!(ALLOCATIONS);
        if *used < (*log).len() {
            (*log)[*used] = Allocation {
                first_slot,
                pages,
                vaddr,
            };
            *used += 1;
        }
    }
}

/// The slot, address and size of the allocation of exactly `pages` frames.
pub fn find_allocation(pages: usize) -> Option<(u64, usize, usize)> {
    // SAFETY: single-core user task; written only during setup.
    unsafe {
        let used = *core::ptr::addr_of_mut!(ALLOCATIONS_USED);
        let log = &*core::ptr::addr_of!(ALLOCATIONS);
        (0..used)
            .find(|index| log[*index].pages == pages)
            .map(|index| {
                let allocation = &log[index];
                (
                    allocation.first_slot,
                    allocation.vaddr,
                    allocation.pages * 0x1000,
                )
            })
    }
}


/// Device-visible physical addresses: `dma_alloc` retypes and maps frames
/// from the service budget; `share` resolves any task memory (driver heap,
/// request headers) through the kernel's self-translation invocation.
pub struct HalImpl;
// SAFETY: dma_alloc hands out freshly retyped, zeroed frames this task
// exclusively owns; share only reports existing mappings to the device.
unsafe impl Hal for HalImpl {
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        static mut NEXT_SLOT: u64 = DMA_SLOT;
        // SAFETY: single-core user task; DMA setup never re-enters.
        unsafe {
            let slot = *core::ptr::addr_of_mut!(NEXT_SLOT);
            if pages == 0 || slot + pages as u64 > DMA_SLOT + DMA_MAX_PAGES as u64 {
                return (0, NonNull::dangling());
            }
            let cnode = CNode(CPtr(INIT_CNODE));
            // One retype call carries at most 32 objects (kernel wire limit),
            // so a 300-page framebuffer is carved chunkwise. Successive calls
            // run the budget watermark forward, the frames stay contiguous,
            // which is what the device's resource backing requires.
            for chunk in (0..pages as u64).step_by(32) {
                let count = (pages as u64 - chunk).min(32);
                if Untyped(CPtr(INIT_UNTYPED))
                    .retype(ObjectType::SmallPage, 0, cnode.0, slot + chunk, count)
                    .is_err()
                {
                    return (0, NonNull::dangling());
                }
            }
            let vaddr = DMA_VA + (slot - DMA_SLOT) as usize * 0x1000;
            let Ok(paddr) = Page(CPtr(slot)).address() else {
                return (0, NonNull::dangling());
            };
            for index in 0..pages {
                // SAFETY: freshly retyped frames, exclusively owned by this task.
                if Page(CPtr(slot + index as u64))
                    .map(
                        CPtr(INIT_VSPACE),
                        vaddr + index * 0x1000,
                        RIGHTS_READ | RIGHTS_WRITE,
                        VM_CACHEABLE | VM_EXECUTE_NEVER,
                    )
                    .is_err()
                {
                    return (0, NonNull::dangling());
                }
            }
            *core::ptr::addr_of_mut!(NEXT_SLOT) = slot + pages as u64;
            record_allocation(slot, pages, vaddr);
            (
                paddr as u64,
                NonNull::new(vaddr as *mut u8).expect("page-aligned vaddr"),
            )
        }
    }

    /// Frames stay mapped until the supervisor revokes the budget on restart;
    /// virtio-drivers only frees when the driver object drops, which here is
    /// service shutdown.
    unsafe fn dma_dealloc(_paddr: PhysAddr, _vaddr: NonNull<u8>, _pages: usize) -> i32 {
        0
    }

    unsafe fn mmio_phys_to_virt(paddr: PhysAddr, _size: usize) -> NonNull<u8> {
        // Only used by the PCI transport; MMIO here is mapped by hand.
        NonNull::new(paddr as usize as *mut u8).expect("non-null mmio")
    }

    unsafe fn share(buffer: NonNull<[u8]>, _direction: BufferDirection) -> PhysAddr {
        let vaddr = buffer.as_ptr() as *mut u8 as usize;
        rstiny::capability::translate(vaddr).unwrap_or(0) as PhysAddr
    }

    unsafe fn unshare(_paddr: PhysAddr, _buffer: NonNull<[u8]>, _direction: BufferDirection) {}
}
