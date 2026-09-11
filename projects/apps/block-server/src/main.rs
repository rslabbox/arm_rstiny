#![no_std]
#![no_main]
//! block-server: the VirtIO MMIO block driver as a supervised user service
//! (docs/disk-driver.md section 6), built on the maintained `virtio-drivers`
//! crate. One bound client at a time; sector data is DMAd directly into a
//! server-owned shared buffer frame granted on BIND. Reads complete through
//! the device IRQ: the supervisor grants an `IrqHandler` cap, the server
//! binds its own Notification and waits instead of polling (docs/irq.md §7).

extern crate alloc;

use core::hint::spin_loop;
use core::ptr::NonNull;

use rstiny::capability::{
    CNode, CPtr, INIT_CNODE, INIT_UNTYPED, INIT_VSPACE, IrqHandler, ObjectType, Page, PageTable,
    RIGHTS_READ, RIGHTS_WRITE, Untyped, VM_CACHEABLE, VM_EXECUTE_NEVER,
};
use rstiny::ipc;
use rstiny_protocol::{Argument, SpawnInfo, block, control, status};
use rstiny_runtime::entry;
use rstiny_server::{Service, logln};
use virtio_drivers::{
    BufferDirection, Error as VirtError, Hal, PhysAddr,
    device::blk::{BlkReq, BlkResp, VirtIOBlk},
    transport::{DeviceType, Transport, mmio::MmioTransport},
};

// One 2 MiB window covers the MMIO mapping, the DMA frames and the shared
// buffer, so a single L3 table from the service budget backs all of them.
const WINDOW_VA: usize = 0x0400_0000;
const MMIO_PAGES: u64 = 4; // 16 KiB VirtIO MMIO window from the device Untyped
const MMIO_VA: usize = WINDOW_VA;
const DMA_VA: usize = WINDOW_VA + 0x8000; // DMA frames handed out by Hal
const DMA_MAX_PAGES: usize = 16;
const BUF_VA: usize = DMA_VA + DMA_MAX_PAGES * 0x1000;
const MMIO_PAGE: u64 = 40;
const TABLE_SLOT: u64 = 44;
const DMA_SLOT: u64 = 45; // + i: DMA frames, handed out sequentially
const BUF_SLOT: u64 = DMA_SLOT + DMA_MAX_PAGES as u64;
const NT_SLOT: u64 = BUF_SLOT + 1; // completion Notification object
const NT_BADGED: u64 = NT_SLOT + 1; // its badged copy, bound to the IRQ line
/// Delivery badge minted onto the bound Notification copy. Non-zero so an
/// unwaited signal merges into the notification bits instead of being lost.
const IRQ_BADGE: u64 = 1;
const SECTORS_PER_BUFFER: u64 = 8;
const SLOT_STRIDE: u64 = 0x200;

/// Bump allocator over a fixed BSS pool: `virtio-drivers` needs an allocator
/// symbol; nothing is freed before the supervisor tears the service down.
const POOL_BYTES: usize = 32 * 1024;
struct Bump;
static mut POOL: [u64; POOL_BYTES / 8] = [0; POOL_BYTES / 8];
static mut POOL_USED: usize = 0;
unsafe impl alloc::alloc::GlobalAlloc for Bump {
    unsafe fn alloc(&self, layout: alloc::alloc::Layout) -> *mut u8 {
        // SAFETY: single-core user task; the cursor is only touched here.
        unsafe {
            let used = core::ptr::addr_of_mut!(POOL_USED);
            let start = core::ptr::addr_of_mut!(POOL) as usize;
            let offset = (*used).next_multiple_of(layout.align().max(8));
            if offset + layout.size() > POOL_BYTES {
                return core::ptr::null_mut();
            }
            *used = offset + layout.size();
            (start + offset) as *mut u8
        }
    }
    unsafe fn dealloc(&self, _pointer: *mut u8, _layout: alloc::alloc::Layout) {}
}
#[global_allocator]
static ALLOCATOR: Bump = Bump;

/// Device-visible physical addresses: `dma_alloc` retypes and maps frames
/// from the service budget; `share` resolves any task memory (driver heap,
/// stack request headers, the shared client buffer) through the kernel's
/// self-translation invocation.
struct HalImpl;
// SAFETY: dma_alloc hands out freshly retyped, zeroed frames this task
// exclusively owns; share only reports existing mappings to the device.
unsafe impl Hal for HalImpl {
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        static mut NEXT_SLOT: u64 = DMA_SLOT;
        // SAFETY: single-core user task; DMA setup never re-enters.
        let slot = unsafe { *core::ptr::addr_of_mut!(NEXT_SLOT) };
        if pages == 0 || slot + pages as u64 > BUF_SLOT {
            return (0, NonNull::dangling());
        }
        let cnode = CNode(CPtr(INIT_CNODE));
        if Untyped(CPtr(INIT_UNTYPED))
            .retype(ObjectType::SmallPage, 0, cnode.0, slot, pages as u64)
            .is_err()
        {
            return (0, NonNull::dangling());
        }
        let vaddr = DMA_VA + (slot - DMA_SLOT) as usize * 0x1000;
        let Ok(paddr) = Page(CPtr(slot)).address() else {
            return (0, NonNull::dangling());
        };
        for index in 0..pages {
            // SAFETY: freshly retyped frames, exclusively owned by this task.
            unsafe {
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
        }
        // SAFETY: single-core user task; DMA setup never re-enters.
        unsafe { *core::ptr::addr_of_mut!(NEXT_SLOT) = slot + pages as u64 };
        (
            paddr as u64,
            NonNull::new(vaddr as *mut u8).expect("page-aligned vaddr"),
        )
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

#[entry]
fn main(argument: Argument) -> ! {
    let Some(service) = Service::init(argument) else {
        loop {
            spin_loop();
        }
    };
    // Dependency-order drill (BLOCK_TEST=fail): exit immediately so fs and
    // appmgr must never start (docs/disk-driver.md section 12, D4).
    if option_env!("BLOCK_TEST").is_some_and(|value| value == "fail") {
        service.exit(1);
    }
    let Some(self_ep) = service
        .extra
        .get(SpawnInfo::SELF_EP)
        .copied()
        .filter(|s| *s != 0)
    else {
        service.exit(2);
    };
    let Some(device_slot) = service
        .extra
        .get(SpawnInfo::DEVICE_SLOT)
        .copied()
        .filter(|s| *s != 0)
    else {
        service.exit(2);
    };
    let cnode = CNode(CPtr(INIT_CNODE));
    // Device registers: the whole window comes from the device Untyped, which
    // the supervisor grants in full (device regions are never split).
    if Untyped(CPtr(device_slot))
        .retype(ObjectType::SmallPage, 0, cnode.0, MMIO_PAGE, MMIO_PAGES)
        .is_err()
    {
        logln!(service, "[block] cannot retype the device window");
        service.exit(3);
    }
    // The covering L3 and the shared buffer frame come from the budget; the
    // buffer is what BIND grants to the client and what the device DMAs into.
    if Untyped(CPtr(INIT_UNTYPED))
        .retype(ObjectType::PageTable, 0, cnode.0, TABLE_SLOT, 1)
        .is_err()
        || Untyped(CPtr(INIT_UNTYPED))
            .retype(ObjectType::SmallPage, 0, cnode.0, BUF_SLOT, 1)
            .is_err()
    {
        logln!(service, "[block] cannot budget the DMA structures");
        service.exit(3);
    }
    // SAFETY: these mappings are exclusive to this task; the shared buffer has
    // no cached alias (the client maps the same frame after BIND).
    unsafe {
        if PageTable(CPtr(TABLE_SLOT))
            .map(vspace_of(), WINDOW_VA & !0x1F_FFFF)
            .is_err()
        {
            logln!(service, "[block] cannot map the covering table");
            service.exit(3);
        }
        for index in 0..MMIO_PAGES {
            if Page(CPtr(MMIO_PAGE + index))
                .map(
                    vspace_of(),
                    MMIO_VA + index as usize * 0x1000,
                    RIGHTS_READ | RIGHTS_WRITE,
                    VM_EXECUTE_NEVER,
                )
                .is_err()
            {
                logln!(service, "[block] cannot map the device window");
                service.exit(3);
            }
        }
        if Page(CPtr(BUF_SLOT))
            .map(
                vspace_of(),
                BUF_VA,
                RIGHTS_READ | RIGHTS_WRITE,
                VM_CACHEABLE | VM_EXECUTE_NEVER,
            )
            .is_err()
        {
            logln!(service, "[block] cannot map the shared buffer");
            service.exit(3);
        }
    }
    let Ok(buffer_phys) = Page(CPtr(BUF_SLOT)).address() else {
        service.exit(3);
    };
    let _ = buffer_phys; // share() resolves it through kernel translation

    // The supervisor grants the device IRQ handler (docs/irq.md §7); without
    // it the driver would have to poll, which this service does not do.
    let irq_slot = service.extra[SpawnInfo::IRQ_SLOT];
    if irq_slot == 0 {
        logln!(service, "[block] no device IRQ handler granted");
        service.exit(2);
    }
    let irq = IrqHandler(CPtr(irq_slot));

    // Probe every 0x200 slot in the granted window: QEMU virt exposes uniform
    // empty slots, and only the one bound to virtio-blk identifies itself.
    let Some(mut device) = probe(MMIO_VA, MMIO_PAGES as usize * 0x1000) else {
        logln!(service, "[block] no virtio-blk device in the window");
        service.exit(4);
    };
    logln!(
        service,
        "[block] device ready, {} sectors",
        device.capacity()
    );
    // Acceptance hook (BLK_TEST=1): read sector 0 through the driver and log
    // capacity plus a checksum the check script compares against the image.
    // Deliberately before the IRQ binding: this exercises the driver without
    // the notification path.
    if option_env!("BLK_TEST").is_some_and(|value| value == "1") {
        // SAFETY: the shared buffer is exclusively mapped by this task.
        let buffer = unsafe { core::slice::from_raw_parts_mut(BUF_VA as *mut u8, 512) };
        match device.read_blocks(0, buffer) {
            Ok(()) => {
                let sum: u32 = buffer.iter().copied().map(u32::from).sum();
                let first: u32 = u32::from_le_bytes(buffer[..4].try_into().expect("512 bytes"));
                logln!(service, "[block] test capacity={}", device.capacity());
                logln!(service, "[block] test sum={sum:#x} head={first:#x}");
            }
            Err(error) => logln!(service, "[block] test read failed: {error:?}"),
        }
    }

    // Bind the completion path: our own Notification, minted with a badge and
    // handed to the kernel through SetNotification. Rebinding recovers a line
    // a crashed predecessor left disabled (docs/irq.md §8).
    //
    // Clear a possibly latched device interrupt first (pre-bind reads, or a
    // crashed predecessor that died mid-completion): the device holds its IRQ
    // line asserted until InterruptACK, and re-enabling an edge line without
    // a fresh edge would never deliver (docs/irq.md §7).
    let _ = device.ack_interrupt();
    let cnode = CNode(CPtr(INIT_CNODE));
    if Untyped(CPtr(INIT_UNTYPED))
        .retype(ObjectType::Notification, 0, cnode.0, NT_SLOT, 1)
        .is_err()
    {
        logln!(service, "[block] cannot budget the completion notification");
        service.exit(5);
    }
    if cnode
        .mint(
            NT_BADGED,
            CPtr(INIT_CNODE),
            NT_SLOT,
            RIGHTS_WRITE,
            IRQ_BADGE,
        )
        .is_err()
    {
        logln!(service, "[block] cannot mint the badged notification");
        service.exit(6);
    }
    if irq.set_notification(CPtr(NT_BADGED)).is_err() {
        logln!(service, "[block] cannot bind the completion interrupt");
        service.exit(7);
    }
    device.enable_interrupts();
    logln!(service, "[block] completion interrupt bound");

    let mut bound: Option<u64> = None;
    loop {
        let Ok(received) = ipc::recv(self_ep) else {
            continue;
        };
        match received.label {
            block::BIND => {
                // Grant the shared buffer to the client; the client must have
                // published a receive spec or the transfer fails on the wire.
                let version = received.word(0);
                if version != block::PROTOCOL_VERSION {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                }
                let reply = ipc::reply_cap(
                    status::OK,
                    &[block::PROTOCOL_VERSION, device.capacity(), SECTOR_SIZE],
                    &[BUF_SLOT],
                );
                if reply.is_ok() {
                    bound = Some(received.badge);
                    logln!(service, "[block] client 0x{:x} bound", received.badge);
                }
            }
            block::READ => {
                if bound != Some(received.badge) {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                }
                let lba = received.word(0) as usize;
                let count = received.word(1) as usize;
                if count == 0 || count > SECTORS_PER_BUFFER as usize {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                }
                // SAFETY: the shared buffer is exclusively mapped by this
                // task; the client reads it only after the reply arrives.
                let buffer =
                    unsafe { core::slice::from_raw_parts_mut(BUF_VA as *mut u8, count * 512) };
                match read_via_interrupt(&mut device, &irq, buffer, lba) {
                    Ok(()) => {
                        let _ = ipc::reply(status::OK, &[count as u64]);
                    }
                    Err(error) => {
                        logln!(service, "[block] read {lba}x{count} failed: {error:?}");
                        let _ = ipc::reply(status::ERROR, &[0]);
                    }
                }
            }
            block::CAPACITY => {
                let _ = ipc::reply(status::OK, &[device.capacity()]);
            }
            block::INFO => {
                let _ = ipc::reply(status::OK, &[SECTOR_SIZE, SECTORS_PER_BUFFER]);
            }
            control::STOP => {
                // Drop the driver before the DMA-visible memory is reclaimed:
                // VirtIOBlk quiesces the queue on drop.
                drop(device);
                let _ = ipc::reply(control::STOP_ACK, &[]);
                service.exit(0);
            }
            _ => {
                let _ = ipc::reply(status::ERROR, &[]);
            }
        }
    }
}

const SECTOR_SIZE: u64 = 512;

fn vspace_of() -> CPtr {
    CPtr(INIT_VSPACE)
}

/// One interrupt-driven read (docs/irq.md §7): submit the request, wait for
/// the completion notification, drain the entire used ring (merged or repeat
/// notifications must not lose completions), clear the device's interrupt
/// status and only then deactivate the line for the next interrupt.
fn read_via_interrupt(
    device: &mut VirtIOBlk<HalImpl, MmioTransport<'static>>,
    irq: &IrqHandler,
    buffer: &mut [u8],
    lba: usize,
) -> Result<(), VirtError> {
    let mut request = BlkReq::default();
    let mut response = BlkResp::default();
    // SAFETY: `request`, `buffer` and `response` are borrowed by the device
    // until `complete_read_blocks` below; this task blocks on the completion
    // notification in between and nothing else touches them.
    let token = unsafe { device.read_blocks_nb(lba, &mut request, buffer, &mut response) }?;
    loop {
        // Badge or merged bits. A notification that raced ahead of the used
        // ring loops back to waiting rather than replying early.
        let _ = ipc::recv(NT_SLOT);
        let mut completed = false;
        while let Some(done) = device.peek_used() {
            if done != token {
                return Err(VirtError::WrongToken);
            }
            // SAFETY: the same buffers `read_blocks_nb` handed to the device;
            // popping the token makes them driver-owned again.
            unsafe {
                device.complete_read_blocks(done, &mut request, buffer, &mut response)?;
            }
            completed = true;
        }
        // Clear the device's interrupt status before deactivating the line:
        // with a still-asserted source the line would re-fire immediately.
        let _ = device.ack_interrupt();
        irq.ack().map_err(|_| VirtError::IoError)?;
        if completed {
            return response.status().into();
        }
    }
}

/// Probe every slot in the granted MMIO window for a virtio-blk device.
fn probe(window: usize, window_size: usize) -> Option<VirtIOBlk<HalImpl, MmioTransport<'static>>> {
    let slots = window_size / SLOT_STRIDE as usize;
    for slot in 0..slots {
        // SAFETY: the window is a device Untyped frame range this task mapped
        // exclusively; every 0x200 slot is a valid VirtIO MMIO region.
        let Ok(transport) = (unsafe {
            MmioTransport::new(
                NonNull::new((window + slot * SLOT_STRIDE as usize) as *mut _)?,
                SLOT_STRIDE as usize,
            )
        }) else {
            continue;
        };
        if transport.device_type() != DeviceType::Block {
            continue;
        }
        return VirtIOBlk::new(transport).ok();
    }
    None
}
