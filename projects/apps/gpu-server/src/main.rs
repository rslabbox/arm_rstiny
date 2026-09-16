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

mod devices;
mod hal;


use rstiny::capability::{
    CNode, CPtr, INIT_CNODE, INIT_UNTYPED, INIT_VSPACE, IrqHandler, ObjectType, Page, PageTable,
    RIGHTS_READ, RIGHTS_WRITE, Untyped, VM_CACHEABLE, VM_EXECUTE_NEVER,
};
use rstiny::ipc::{self, ReceiveSpec};
use rstiny_alloc::Heap;
use rstiny_protocol::{SpawnInfo, control, gpu, status};
use rstiny_runtime::entry;
use rstiny_server::{Service, logln};
use virtio_drivers::device::input::{InputEvent, VirtIOInput};
use virtio_drivers::transport::mmio::MmioTransport;

use hal::{find_allocation, HalImpl, IRQ_BADGE_GPU, IRQ_BADGE_INPUT, MMIO_PAGE, MMIO_PAGES, MMIO_VA, NT_GPU, NT_GPU_BADGED, NT_INPUT, NT_INPUT_BADGED, REL_SLOT, TABLE_SLOT, WINDOW_VA};

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

/// Task heap: rstiny-alloc (interpreter-app.md 决策 B) — the same first-fit
/// allocator the C staticlib exports, shared by every Rust task. Frees are
/// reused, and growth comes from this task's own Untyped budget.
#[global_allocator]
static HEAP: Heap = Heap;

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
fn main(service: Service) -> ! {
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
    let Some(mut device) = devices::probe_gpu(MMIO_VA, MMIO_PAGES as usize * 0x1000) else {
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
    let mut inputs = devices::probe_inputs(MMIO_VA, MMIO_PAGES as usize * 0x1000);
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
    // READY was announced on entry (standard semantics): clients that bind
    // before this point block in IPC until the recv loop below serves them.
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
