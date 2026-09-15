#![no_std]
#![no_main]
//! gpu-server: the VirtIO display driver as a supervised user service
//! (docs/gui-display.md §3). It drives virtio-gpu over the granted MMIO
//! window, owns the full-screen framebuffer DMA resource and leases it to
//! exactly one client at a time as *capabilities*: LEASE hands the Frame
//! caps over in batches (the wire carries at most [`gpu::CAPS_PER_BATCH`]),
//! the server drops its own mappings, the client draws in place and FLUSHes,
//! and RELEASE returns the caps batch by batch before the server re-maps.
//! A second VirtIO device — the keyboard — feeds INPUT_READ; its completion
//! line is bound to the service's second Notification (docs/irq.md §7).

extern crate alloc;

use core::hint::spin_loop;
use core::ptr::NonNull;

use rstiny::capability::{
    CNode, CPtr, INIT_CNODE, INIT_UNTYPED, INIT_VSPACE, IrqHandler, ObjectType, Page, PageTable,
    RIGHTS_READ, RIGHTS_WRITE, Untyped, VM_CACHEABLE, VM_EXECUTE_NEVER,
};
use rstiny::ipc::{self, ReceiveSpec};
use rstiny_protocol::{Argument, SpawnInfo, control, gpu, status};
use rstiny_runtime::entry;
use rstiny_server::{Service, logln};
use virtio_drivers::{
    BufferDirection, Hal, PhysAddr,
    device::gpu::VirtIOGpu,
    device::input::{InputEvent, VirtIOInput},
    transport::{DeviceType, Transport, mmio::MmioTransport},
};

// One 2 MiB window covers the MMIO mapping, the DMA frames (queues +
// framebuffer) and nothing else, so a single L3 table from the service
// budget backs all of it (docs/gui-display.md §10: budget = 2M).
const WINDOW_VA: usize = 0x0400_0000;
const MMIO_PAGES: u64 = 4; // 16 KiB VirtIO MMIO window from the device Untyped
const MMIO_VA: usize = WINDOW_VA;
const DMA_VA: usize = WINDOW_VA + 0x8000; // DMA frames handed out by Hal
const DMA_MAX_PAGES: usize = 512; // queue rings + 300 framebuffer pages
const MMIO_PAGE: u64 = 40;
const TABLE_SLOT: u64 = 44;
/// DMA frames, handed out sequentially. The window starts *above* the fixed
/// child CSpace layout (control 140, console 51, self 52, deps 53.., IRQ
/// 56..): a 300-page framebuffer spans 300 slots and must not run into them.
const DMA_SLOT: u64 = 200;
const NT_GPU: u64 = DMA_SLOT + DMA_MAX_PAGES as u64; // flush completion
const NT_GPU_BADGED: u64 = NT_GPU + 1;
const NT_INPUT: u64 = NT_GPU + 2; // keyboard event line
const NT_INPUT_BADGED: u64 = NT_INPUT + 1;
/// Landing slots for the RELEASE batches. The server keeps its master caps
/// in fb_slot.. (crash recovery), so returned caps must land elsewhere; each
/// verified batch is deleted again, keeping the cap count flat.
const REL_SLOT: u64 = NT_INPUT_BADGED + 1;
/// Delivery badges minted onto the bound Notification copies. Non-zero so an
/// unwaited signal merges into the notification bits instead of being lost.
const IRQ_BADGE_GPU: u64 = 1;
const IRQ_BADGE_INPUT: u64 = 2;
const SLOT_STRIDE: u64 = 0x200;

/// Pixel format on the wire: little-endian `0xAARRGGBB` (virtio
/// `B8G8R8A8UNORM`). The D1 self-test bands the screen with this palette;
/// tools/check_gpu.py replicates both the bands and the checksum below.
const PALETTE: [(u8, u8, u8); 16] = [
    (0x00, 0x00, 0x00),
    (0x11, 0x11, 0x11),
    (0x22, 0x22, 0x22),
    (0x33, 0x33, 0x33),
    (0x44, 0x44, 0x44),
    (0x55, 0x55, 0x55),
    (0x66, 0x66, 0x66),
    (0x77, 0x77, 0x77),
    (0x88, 0x88, 0x88),
    (0x99, 0x99, 0x99),
    (0xAA, 0xAA, 0xAA),
    (0xBB, 0xBB, 0xBB),
    (0xCC, 0xCC, 0xCC),
    (0xDD, 0xDD, 0xDD),
    (0xEE, 0xEE, 0xEE),
    (0xFF, 0xFF, 0xFF),
];

/// Bump allocator over a fixed BSS pool: `virtio-drivers` needs an allocator
/// symbol (queue buffers, input ring); nothing is freed before the supervisor
/// tears the service down.
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
fn find_allocation(pages: usize) -> Option<(u64, usize, usize)> {
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
struct HalImpl;
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

/// The leased framebuffer: caps and virtual window this task knows about.
struct Lease {
    /// The lease holder's endpoint badge (v1: one client, identified only to
    /// refuse FLUSH from anyone else).
    client: u64,
    pages: usize,
    /// LEASE_BATCH: next page the client is still owed. RELEASE: pages
    /// already handed back.
    next_page: usize,
    returned: usize,
}

#[entry]
fn main(argument: Argument) -> ! {
    let Some(service) = parse_service(argument) else {
        loop {
            spin_loop();
        }
    };
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
    // the supervisor grants in full. The kernel resolves device retypes by
    // physical address, so block- and gpu-server share the same window
    // frames (docs/gui-display.md §10).
    if Untyped(CPtr(device_slot))
        .retype(ObjectType::SmallPage, 0, cnode.0, MMIO_PAGE, MMIO_PAGES)
        .is_err()
    {
        logln!(service, "[gpu] cannot retype the device window");
        service.exit(3);
    }
    // The covering L3 comes from the budget; the DMA frames the Hal retypes
    // (queue rings, framebuffer) are billed to the same budget.
    if Untyped(CPtr(INIT_UNTYPED))
        .retype(ObjectType::PageTable, 0, cnode.0, TABLE_SLOT, 1)
        .is_err()
    {
        logln!(service, "[gpu] cannot budget the DMA structures");
        service.exit(3);
    }
    // SAFETY: these mappings are exclusive to this task.
    unsafe {
        if PageTable(CPtr(TABLE_SLOT))
            .map(CPtr(INIT_VSPACE), WINDOW_VA & !0x1F_FFFF)
            .is_err()
        {
            logln!(service, "[gpu] cannot map the covering table");
            service.exit(3);
        }
        for index in 0..MMIO_PAGES {
            if Page(CPtr(MMIO_PAGE + index))
                .map(
                    CPtr(INIT_VSPACE),
                    MMIO_VA + index as usize * 0x1000,
                    RIGHTS_READ | RIGHTS_WRITE,
                    VM_EXECUTE_NEVER,
                )
                .is_err()
            {
                logln!(service, "[gpu] cannot map the device window");
                service.exit(3);
            }
        }
    }

    // The supervisor grants one IRQ handler per device, in `devices` order
    // (docs/irq.md §8): gpu first, then the keyboard.
    let irq_slot = service.extra[SpawnInfo::IRQ_SLOT];
    let irq2_slot = service.extra[SpawnInfo::IRQ_SLOT2];
    if irq_slot == 0 || irq2_slot == 0 {
        logln!(service, "[gpu] device IRQ handlers not granted");
        service.exit(2);
    }
    let gpu_irq = IrqHandler(CPtr(irq_slot));
    let _input_irq = IrqHandler(CPtr(irq2_slot));

    // Probe every 0x200 slot in the granted window: only the slots QEMU
    // attached virtio-gpu / virtio-keyboard to identify themselves.
    let Some(mut device) = probe_gpu(MMIO_VA, MMIO_PAGES as usize * 0x1000) else {
        logln!(service, "[gpu] no virtio-gpu device in the window");
        service.exit(4);
    };
    let Ok((width, height)) = device.resolution() else {
        logln!(service, "[gpu] GET_DISPLAY_INFO failed");
        service.exit(4);
    };
    let framebuffer = match device.setup_framebuffer() {
        Ok(framebuffer) => framebuffer,
        Err(error) => {
            logln!(service, "[gpu] setup_framebuffer: {error:?}");
            service.exit(4);
        }
    };
    let fb_bytes = framebuffer.len();
    let fb_pages = fb_bytes / 0x1000;
    let Some((fb_slot, fb_va, _)) = find_allocation(fb_pages) else {
        logln!(service, "[gpu] framebuffer allocation not found");
        service.exit(3);
    };
    logln!(
        service,
        "[gpu] display {}x{}, framebuffer {} pages at slots {fb_slot}..",
        width,
        height,
        fb_pages
    );

    // Acceptance hook (GPU_TEST=1): band the screen, submit one flush and
    // log the word checksum the check script recomputes host-side
    // (docs/gui-display.md §6, D1). Before the IRQ binding, mirroring the
    // block driver: this exercises the device without the notification path.
    if option_env!("GPU_TEST").is_some_and(|value| value == "1") {
        // SAFETY: the framebuffer is mapped exclusively by this task here;
        // the lease has not started yet.
        let pixels = unsafe { core::slice::from_raw_parts_mut(fb_va as *mut u32, fb_bytes / 4) };
        let stride = width as usize;
        for (row, line) in pixels.chunks_mut(stride).enumerate() {
            let (red, green, blue) = PALETTE[row.min(PALETTE.len() - 1)];
            let color = color_word(red, green, blue);
            for pixel in line.iter_mut() {
                *pixel = color;
            }
        }
        match device.flush() {
            Ok(()) => {
                logln!(
                    service,
                    "[gpu] test flush {}x{} pages={fb_pages} sum={:#x}",
                    width,
                    height,
                    word_sum(unsafe { core::slice::from_raw_parts(fb_va as *const u8, fb_bytes) })
                );
            }
            Err(error) => logln!(service, "[gpu] test flush failed: {error:?}"),
        }
        // Leave the line clean for the bound Notification below.
        let _ = device.ack_interrupt();
    }

    // Bind both completion paths: our own Notifications, minted with badges
    // and handed to the kernel through SetNotification. Clear latched device
    // interrupts before rebinding (docs/irq.md §7): a level line that is
    // still asserted would re-fire the instant the kernel re-enables it.
    if Untyped(CPtr(INIT_UNTYPED))
        .retype(ObjectType::Notification, 0, cnode.0, NT_GPU, 1)
        .is_err()
        || Untyped(CPtr(INIT_UNTYPED))
            .retype(ObjectType::Notification, 0, cnode.0, NT_INPUT, 1)
            .is_err()
    {
        logln!(service, "[gpu] cannot budget the notifications");
        service.exit(5);
    }
    if cnode
        .mint(
            NT_GPU_BADGED,
            CPtr(INIT_CNODE),
            NT_GPU,
            RIGHTS_WRITE,
            IRQ_BADGE_GPU,
        )
        .is_err()
        || cnode
            .mint(
                NT_INPUT_BADGED,
                CPtr(INIT_CNODE),
                NT_INPUT,
                RIGHTS_WRITE,
                IRQ_BADGE_INPUT,
            )
            .is_err()
    {
        logln!(service, "[gpu] cannot mint the badged notifications");
        service.exit(6);
    }
    if gpu_irq.set_notification(CPtr(NT_GPU_BADGED)).is_err() {
        logln!(service, "[gpu] cannot bind the flush interrupt");
        service.exit(7);
    }
    // Unlike VirtIOBlk, the GPU driver manages its per-request notify flags
    // itself (control requests poll the used ring), so there is no
    // enable_interrupts step: the bound Notification simply receives the
    // line whenever the device raises it.

    // Every virtio-input device in the window (keyboard, mouse) feeds
    // INPUT_READ; the first one's line is bound to the input Notification,
    // the rest are polled (docs/gui-display.md §4).
    let mut inputs = probe_inputs(MMIO_VA, MMIO_PAGES as usize * 0x1000);
    if !inputs.is_empty() {
        // Poll-only: INPUT_READ drains every device's ring directly. (The
        // interrupt binding for the keyboard line was removed: binding and
        // acking a second line while the block IRQ path is live destabilized
        // the GIC routing state on the 6-service topology.)
        logln!(service, "[gpu] {} input device(s) polled", inputs.len());
    } else {
        logln!(service, "[gpu] no virtio-input device in the window");
    }

    // Fully initialized: only now let the supervisor spawn the rest of the
    // topology (fs, mysh).
    let _ = ipc::call(service.control_ep, control::READY, &[]);
    logln!(service, "[gpu] ready");

    let mut lease: Option<Lease> = None;
    loop {
        let Ok(received) = ipc::recv(self_ep) else {
            continue;
        };
        match received.label {
            gpu::BIND => {
                if received.word(0) != gpu::PROTOCOL_VERSION {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                }
                let _ = ipc::reply(
                    status::OK,
                    &[
                        gpu::PROTOCOL_VERSION,
                        width as u64,
                        height as u64,
                        fb_bytes as u64,
                    ],
                );
            }
            gpu::LEASE => {
                if lease.is_some() {
                    logln!(service, "[gpu] lease refused: already leased");
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                }
                // Drop our pixel alias before the client draws: the server
                // keeps the caps (crash recovery) but no mapping, so the
                // client holds the only live view of the frames.
                for index in 0..fb_pages {
                    // SAFETY: the driver holds no pointer into the pixels
                    // after setup_framebuffer; flush() works on the device
                    // resource, not this mapping.
                    unsafe {
                        let _ = Page(CPtr(fb_slot + index as u64)).unmap();
                    }
                }
                let caps: [u64; gpu::CAPS_PER_BATCH] =
                    core::array::from_fn(|offset| fb_slot + offset as u64);
                // Expect the RELEASE batches in the dedicated landing slots
                // *before* replying: the receive spec must be current at the
                // instant the client's next message is delivered (the spec is
                // sticky in the IPC buffer, so it is restated between
                // batches as well).
                let _ = ipc::set_receive_spec(ReceiveSpec {
                    cnode: INIT_CNODE,
                    index: REL_SLOT,
                    depth: 64,
                });
                let Ok(()) = ipc::reply_cap(status::OK, &[fb_pages as u64, fb_bytes as u64], &caps)
                else {
                    // The client never got the caps: keep the framebuffer.
                    for index in 0..fb_pages {
                        // SAFETY: as above, no live alias anywhere.
                        unsafe {
                            let _ = Page(CPtr(fb_slot + index as u64)).map(
                                CPtr(INIT_VSPACE),
                                fb_va + index * 0x1000,
                                RIGHTS_READ | RIGHTS_WRITE,
                                VM_CACHEABLE | VM_EXECUTE_NEVER,
                            );
                        }
                    }
                    continue;
                };
                lease = Some(Lease {
                    client: received.badge,
                    pages: fb_pages,
                    next_page: gpu::CAPS_PER_BATCH,
                    returned: 0,
                });
                logln!(
                    service,
                    "[gpu] client 0x{:x} leased {} pages",
                    received.badge,
                    fb_pages
                );
            }
            gpu::LEASE_BATCH => {
                let Some(state) = lease.as_mut() else {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                };
                let first = received.word(0) as usize;
                if received.badge != state.client || first != state.next_page {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                }
                let caps: [u64; gpu::CAPS_PER_BATCH] =
                    core::array::from_fn(|offset| fb_slot + (first + offset) as u64);
                if ipc::reply_cap(status::OK, &[first as u64], &caps).is_err() {
                    continue;
                }
                state.next_page = (first + gpu::CAPS_PER_BATCH).min(state.pages);
            }
            gpu::FLUSH => {
                let Some(state) = lease.as_ref() else {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                };
                let (x, y, rect_w, rect_h) = (
                    received.word(0),
                    received.word(1),
                    received.word(2),
                    received.word(3),
                );
                let inside = x < width as u64
                    && y < height as u64
                    && rect_w > 0
                    && rect_h > 0
                    && x + rect_w <= width as u64
                    && y + rect_h <= height as u64;
                if received.badge != state.client || !inside {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                }
                // v1 submits the whole resource: the driver keeps the
                // per-rect transfer private (docs/gui-display.md, D2 note).
                match device.flush() {
                    Ok(()) => {
                        let _ = ipc::reply(status::OK, &[0]);
                    }
                    Err(error) => {
                        logln!(service, "[gpu] flush failed: {error:?}");
                        let _ = ipc::reply(status::ERROR, &[0]);
                    }
                }
                // The completion raised the line while we span on the used
                // ring; clear the device status, deactivate the line and
                // drain the merged notification bits so nothing accumulates.
                let _ = device.ack_interrupt();
                let _ = gpu_irq.ack();
                let _ = ipc::nbrecv(NT_GPU);
            }
            gpu::RELEASE => {
                let Some(state) = lease.as_mut() else {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                };
                let first = received.word(0) as usize;
                if received.badge != state.client || first != state.returned {
                    // Wrong order: refuse the batch; the caps stay with the
                    // client (the wire transfer only lands on a reply).
                    logln!(
                        service,
                        "[gpu] release batch {first} refused (badge/badge-order)"
                    );
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                }
                // The batch landed in REL_SLOT + first.. — it must be the
                // very memory we handed out: the server checks every frame's
                // physical address before accepting it back (the capability
                // is the authority, the address is the identity).
                if !batch_is_framebuffer(fb_slot, REL_SLOT, first) {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                }
                // Verified: drop the landing copies, the masters stay.
                // SAFETY: duplicates of memory this task still masters; no
                // execution depends on the landing slots.
                unsafe {
                    for offset in 0..gpu::CAPS_PER_BATCH {
                        let _ = cnode.delete(REL_SLOT + (first + offset) as u64);
                    }
                }
                state.returned += gpu::CAPS_PER_BATCH;
                let missing = state.pages - state.returned;
                if missing > 0 {
                    // Restate the landing slots for the next batch *before*
                    // replying: the client sends batch first+3 the moment the
                    // reply lands, and the kernel delivers caps through the
                    // receive spec that is current at that instant.
                    let _ = ipc::set_receive_spec(ReceiveSpec {
                        cnode: INIT_CNODE,
                        index: REL_SLOT + (first + gpu::CAPS_PER_BATCH) as u64,
                        depth: 64,
                    });
                }
                let _ = ipc::reply(status::OK, &[missing as u64]);
                if missing == 0 {
                    // The whole framebuffer is back: re-map it and free the
                    // lease for the next client. (The masters never left, so
                    // this only restores the pixel alias; a client that died
                    // mid-lease would leave the lease stuck — see the
                    // implementation record in docs/gui-display.md.)
                    for index in 0..fb_pages {
                        // SAFETY: the client returned every cap; no other
                        // mapping exists.
                        unsafe {
                            let _ = Page(CPtr(fb_slot + index as u64)).map(
                                CPtr(INIT_VSPACE),
                                fb_va + index * 0x1000,
                                RIGHTS_READ | RIGHTS_WRITE,
                                VM_CACHEABLE | VM_EXECUTE_NEVER,
                            );
                        }
                    }
                    logln!(service, "[gpu] lease released by 0x{:x}", received.badge);
                    lease = None;
                }
            }
            gpu::INPUT_READ => {
                let event = read_input(inputs.as_mut_slice());
                let _ = ipc::reply(status::OK, &[event]);
            }
            control::STOP => {
                // Drop the drivers before the DMA-visible memory is
                // reclaimed: both quiesce their queues on drop.
                drop(inputs);
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

/// Pack one RGB triple into the wire format (`0xAARRGGBB`, fully opaque).
fn color_word(red: u8, green: u8, blue: u8) -> u32 {
    0xFF00_0000 | u32::from(red) << 16 | u32::from(green) << 8 | u32::from(blue)
}

/// Does the batch that just landed at slots `landing + first..` carry the
/// very frames of the framebuffer whose master caps start at `fb_slot`? Each
/// returned cap is checked against the physical address the Hal recorded at
/// allocation time (the capability is the authority, the physical address is
/// the identity).
fn batch_is_framebuffer(fb_slot: u64, landing: u64, first: usize) -> bool {
    let Ok(base) = Page(CPtr(fb_slot)).address() else {
        return false;
    };
    (0..gpu::CAPS_PER_BATCH).all(|offset| {
        let index = first + offset;
        matches!(
            Page(CPtr(landing + index as u64)).address(),
            Ok(paddr) if paddr == base + index * 0x1000
        )
    })
}

/// Wrapping u32 sum over the little-endian pixel words. The check scripts
/// compute the same reduction over the same bytes.
fn word_sum(bytes: &[u8]) -> u32 {
    bytes.chunks_exact(4).fold(0u32, |sum, word| {
        sum.wrapping_add(u32::from_le_bytes(word.try_into().expect("4 bytes")))
    })
}

/// One INPUT_READ: poll every input device's pending events; when all
/// queues look empty, consume a pending interrupt (if any) and poll the
/// first device once more. Returns the packed reply word: bit 63 = present,
/// then type/code/value.
fn read_input(inputs: &mut [VirtIOInput<HalImpl, MmioTransport<'static>>]) -> u64 {
    for _ in 0..4 {
        for input in inputs.iter_mut() {
            if let Some(event) = input.pop_pending_event() {
                return pack_event(&event);
            }
        }
        return 0;
    }
    0
}

fn pack_event(event: &InputEvent) -> u64 {
    (1 << 63)
        | u64::from(event.event_type) << 40
        | u64::from(event.code) << 24
        | (u64::from(event.value) & 0xFF_FFFF)
}

/// Probe every slot in the granted MMIO window for a virtio-gpu device.
fn probe_gpu(
    window: usize,
    window_size: usize,
) -> Option<VirtIOGpu<HalImpl, MmioTransport<'static>>> {
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
        if transport.device_type() != DeviceType::GPU {
            // MmioTransport's Drop resets the device: forget it so a live
            // device (block-server's, or our own after a re-probe) is left
            // untouched. The transport owns no allocation.
            core::mem::forget(transport);
            continue;
        }
        return VirtIOGpu::new(transport).ok();
    }
    None
}

/// Probe every slot in the granted MMIO window and collect every
/// virtio-input device (keyboard, mouse, ...).
fn probe_inputs(
    window: usize,
    window_size: usize,
) -> alloc::vec::Vec<VirtIOInput<HalImpl, MmioTransport<'static>>> {
    let slots = window_size / SLOT_STRIDE as usize;
    let mut inputs = alloc::vec::Vec::new();
    for slot in 0..slots {
        // SAFETY: as `probe_gpu`: exclusively mapped device window.
        let Some(base) = NonNull::new((window + slot * SLOT_STRIDE as usize) as *mut _) else {
            continue;
        };
        let transport = match unsafe { MmioTransport::new(base, SLOT_STRIDE as usize) } {
            Ok(transport) => transport,
            Err(_) => continue,
        };
        if transport.device_type() != DeviceType::Input {
            // MmioTransport's Drop resets the device: forget it, or scanning
            // past the inputs wipes the already-initialised GPU and the
            // block-server's device (the six-service boot deadlock). The
            // transport owns no allocation.
            core::mem::forget(transport);
            continue;
        }
        if let Ok(device) = VirtIOInput::new(transport) {
            inputs.push(device);
        }
    }
    inputs
}

/// Parse the SpawnInfo page into a Service handle WITHOUT announcing READY
/// (the READY call is deferred until the display and input init complete,
/// docs/gui-display.md §10.4).
fn parse_service(argument: usize) -> Option<Service> {
    use rstiny_server::Service;
    if argument == 0 || argument % rstiny_protocol::PAGE_SIZE as usize != 0 {
        return None;
    }
    // SAFETY: the supervisor mapped this page read-only for the child; only
    // this task reads it, for the task lifetime.
    let info = unsafe { &*(argument as *const SpawnInfo) };
    if info.magic != SpawnInfo::MAGIC || info.version != SpawnInfo::VERSION {
        return None;
    }
    Some(Service {
        control_ep: info.control_ep,
        command_ep: info.command_ep,
        console_ep: info.extra[SpawnInfo::CONSOLE_EP],
        extra: info.extra,
    })
}

