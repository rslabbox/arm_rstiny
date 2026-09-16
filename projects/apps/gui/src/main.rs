#![no_std]
#![no_main]
//! gui: the GPU demonstration client (docs/gui-display.md §6, D2-D4). It
//! binds the gpu-server, leases the whole framebuffer as capabilities,
//! draws a scene in place with `rstiny-gui`, FLUSHes, logs the word
//! checksum the acceptance recomputes host-side, and RELEASEs the lease
//! batch by batch (the caps travel back to the server).
//!
//! Scenes (`./gui <scene>`; argv[0] is the first shell token, 决策 H):
//! - `bars`   eight color bars (D2, no font involved);
//! - `text`   bitmap-font text lines (D3);
//! - `scroll` text lines then two terminal scrolls (D3);
//! - `keys N` wait for N key presses and log them (D4).

use core::hint::spin_loop;

use rstiny::capability::{
    CNode, CPtr, INIT_CNODE, INIT_UNTYPED, INIT_VSPACE, ObjectType, Page, PageTable, RIGHTS_READ,
    RIGHTS_WRITE, Untyped, VM_CACHEABLE, VM_EXECUTE_NEVER,
};
use rstiny::ipc::{self, ReceiveSpec};
mod wm;

use rstiny_gui::{Canvas, rgb, word_sum};
use rstiny_protocol::{Argument, ArgvBlock, SpawnInfo, gpu, status};
use rstiny_runtime::entry;
use rstiny_server::{Service, logln};

// The framebuffer window: 2 MiB of this task's address space, one covering
// L3 from the child budget; 640x480x4 ≈ 1.2 MiB fits with room to spare.
const WINDOW_VA: usize = 0x0400_0000;
const TABLE_SLOT: u64 = 44;
/// Landing slots for the LEASE batches (the receive spec is restated before
/// every call): 300 frames need 100 batches of 3 caps.
const RECV_BASE: u64 = 400;
const MAX_ARGS: usize = 4;
/// INPUT_READ poll cadence while waiting for keys (ms) and the give-up count.
const KEY_POLL_MS: u64 = 100;
const KEY_POLLS_MAX: usize = 300;

/// The bar palette of the `bars` scene; check_gpu.py replicates the layout.
const BARS: [(u8, u8, u8); 8] = [
    (0xE0, 0x20, 0x20),
    (0x20, 0xE0, 0x20),
    (0x20, 0x20, 0xE0),
    (0xE0, 0xE0, 0x20),
    (0x20, 0xE0, 0xE0),
    (0xE0, 0x20, 0xE0),
    (0xF0, 0xF0, 0xF0),
    (0x60, 0x60, 0x60),
];

#[entry]
fn main(argument: Argument) -> ! {
    let Some(service) = Service::init(argument) else {
        loop {
            spin_loop();
        }
    };
    let args = parse_argv(argument);
    // Dependency endpoints are granted only to arg-taking programs (决策 I):
    // `./gui` bare would never be able to reach gpu-server, so treat an
    // omitted scene as a usage error instead of a silent default.
    if args.is_empty() {
        logln!(service, "[gui] usage: ./gui bars|text|scroll|wm|keys N");
        logln!(service, "[gui] (the scene argument is what makes mysh grant the gpu endpoint)");
        service.exit(2);
    }
    let scene = args.first().unwrap_or("bars");
    let Some(gpu_ep) = service
        .extra
        .get(SpawnInfo::DEP_EP_BASE + 1)
        .copied()
        .filter(|slot| *slot != 0)
    else {
        logln!(service, "[gui] no gpu endpoint granted");
        service.exit(2);
    };
    // Parameter page → canvas window.
    if Untyped(CPtr(INIT_UNTYPED))
        .retype(
            ObjectType::PageTable,
            0,
            CNode(CPtr(INIT_CNODE)).0,
            TABLE_SLOT,
            1,
        )
        .is_err()
    {
        logln!(service, "[gui] cannot budget the covering table");
        service.exit(3);
    }
    // The covering L3 maps an address range nothing else in this task uses.
    if PageTable(CPtr(TABLE_SLOT))
        .map(CPtr(INIT_VSPACE), WINDOW_VA & !0x1F_FFFF)
        .is_err()
    {
        logln!(service, "[gui] cannot map the covering table");
        service.exit(3);
    }
    let Some(received) = ipc::call(gpu_ep, gpu::BIND, &[gpu::PROTOCOL_VERSION])
        .ok()
        .filter(|reply| reply.label == status::OK && reply.word(0) == gpu::PROTOCOL_VERSION)
    else {
        logln!(service, "[gui] BIND failed");
        service.exit(3);
    };
    let (width, height, fb_bytes) = (
        received.word(1) as usize,
        received.word(2) as usize,
        received.word(3) as usize,
    );
    let pages = fb_bytes / 0x1000;
    if !lease(&service, gpu_ep, pages) {
        logln!(service, "[gui] LEASE failed");
        service.exit(4);
    }
    logln!(
        service,
        "[gui] leased {}x{} ({} pages)",
        width,
        height,
        pages
    );

    // SAFETY: exactly the leased frames are mapped at WINDOW_VA now, and no
    // other task or object touches them while the lease is held.
    let mut canvas = unsafe { Canvas::new(WINDOW_VA as *mut u8, width, height) };
    match scene {
        "bars" => draw_bars(&mut canvas, width, height),
        "text" => draw_text_scene(&mut canvas, width, height),
        "scroll" => draw_scroll_scene(&mut canvas, width, height),
        "wm" => {
            wm::run(&service, &mut canvas, gpu_ep, width, height);
        }
        "keys" => {
            let expected: usize = args
                .get(1)
                .and_then(|count| count.parse().ok())
                .unwrap_or(3);
            draw_keys_scene(&service, &mut canvas, gpu_ep, width, height, expected)
        }
        other => {
            logln!(service, "[gui] unknown scene: {}", other);
            service.exit(5);
        }
    }
    let checksum = word_sum(unsafe {
        // SAFETY: our exclusive leased window, read-only here.
        core::slice::from_raw_parts(WINDOW_VA as *const u8, fb_bytes)
    });
    logln!(service, "[gui] {scene} {width}x{height} sum={checksum:#x}");

    if scene != "keys" {
        // Submit the whole scene once, then hand every frame back.
        if ipc::call(gpu_ep, gpu::FLUSH, &[0, 0, width as u64, height as u64])
            .is_ok_and(|reply| reply.label == status::OK)
        {
            logln!(service, "[gui] flushed");
        } else {
            logln!(service, "[gui] FLUSH failed");
        }
    }
    if release(&service, gpu_ep, pages) {
        logln!(service, "[gui] released");
    }
    service.exit(0)
}

/// Parse the [`ArgvBlock`] that follows the parameter page (决策 H). The
/// strings are NUL-terminated and live in the read-only info page.
fn parse_argv(argument: usize) -> heapless_args::Args<MAX_ARGS> {
    let mut args = heapless_args::Args::new();
    if argument == 0 {
        return args;
    }
    // SAFETY: the supervisor mapped this page read-only; the block layout is
    // fixed by the loader (SpawnInfo, then ArgvBlock, then the strings).
    let base = argument + core::mem::size_of::<SpawnInfo>();
    let block = unsafe { core::ptr::read(base as *const ArgvBlock) };
    if block.magic != ArgvBlock::MAGIC || block.argc == 0 {
        return args;
    }
    let mut cursor = base + core::mem::size_of::<ArgvBlock>();
    for _ in 0..block.argc.min(MAX_ARGS as u64) {
        // SAFETY: NUL-terminated by the loader, inside the same page.
        let string = unsafe { core::ffi::CStr::from_ptr(cursor as *const core::ffi::c_char) };
        if let Ok(text) = string.to_str() {
            args.push(text);
        }
        cursor += string.count_bytes() + 1;
    }
    args
}

/// A tiny fixed-capacity argv holder (no allocator in this task).
mod heapless_args {
    pub struct Args<const N: usize> {
        slots: [&'static str; N],
        used: usize,
    }
    impl<const N: usize> Args<N> {
        pub fn new() -> Self {
            Args {
                slots: [""; N],
                used: 0,
            }
        }
        pub fn push(&mut self, value: &'static str) {
            if self.used < N {
                self.slots[self.used] = value;
                self.used += 1;
            }
        }
        pub fn first(&self) -> Option<&'static str> {
            self.slots.first().copied()
        }
        pub fn get(&self, index: usize) -> Option<&'static str> {
            self.slots.get(index).copied()
        }
        /// The pushed-argument count: `first()` always returns the slot zero
        /// placeholder, so emptiness must come from `used`.
        pub fn is_empty(&self) -> bool {
            self.used == 0
        }
    }
}

/// LEASE + LEASE_BATCH: receive all `pages` Frame caps and map them
/// contiguously at [`WINDOW_VA`]. Returns false if any batch fails.
fn lease(service: &Service, gpu_ep: u64, pages: usize) -> bool {
    for first in (0..pages).step_by(gpu::CAPS_PER_BATCH) {
        if ipc::set_receive_spec(ReceiveSpec {
            cnode: INIT_CNODE,
            index: RECV_BASE + first as u64,
            depth: 64,
        })
        .is_err()
        {
            logln!(service, "[gui] lease: receive spec refused at {first}");
            return false;
        }
        let reply = if first == 0 {
            ipc::call(gpu_ep, gpu::LEASE, &[])
        } else {
            ipc::call(gpu_ep, gpu::LEASE_BATCH, &[first as u64])
        };
        let Ok(reply) = reply else {
            logln!(service, "[gui] lease: batch {first} call failed");
            return false;
        };
        if reply.label != status::OK {
            logln!(service, "[gui] lease: batch {first} refused");
            return false;
        }
    }
    // SAFETY: the caps are exclusively ours (the server kept no mapping);
    // the window covers exactly `pages` frames.
    unsafe {
        for page in 0..pages {
            if Page(CPtr(RECV_BASE + page as u64))
                .map(
                    CPtr(INIT_VSPACE),
                    WINDOW_VA + page * 0x1000,
                    RIGHTS_READ | RIGHTS_WRITE,
                    VM_CACHEABLE | VM_EXECUTE_NEVER,
                )
                .is_err()
            {
                return false;
            }
        }
    }
    true
}

/// RELEASE: return every batch in order; the last one ends the lease.
fn release(service: &Service, gpu_ep: u64, pages: usize) -> bool {
    for first in (0..pages).step_by(gpu::CAPS_PER_BATCH) {
        let caps: [u64; gpu::CAPS_PER_BATCH] =
            core::array::from_fn(|offset| RECV_BASE + (first + offset) as u64);
        let Ok(reply) = ipc::call_cap(gpu_ep, gpu::RELEASE, &[first as u64], &caps) else {
            logln!(service, "[gui] release: batch {first} call failed");
            return false;
        };
        if reply.label != status::OK {
            logln!(service, "[gui] release: batch {first} refused");
            return false;
        }
    }
    true
}

/// D2: eight color bars across the middle band of the screen.
fn draw_bars(canvas: &mut Canvas, width: usize, height: usize) {
    canvas.fill_rect(0, 0, width, height, rgb(0x10, 0x10, 0x30));
    let band_top = height / 8;
    let band_bottom = height - band_top;
    for (index, (red, green, blue)) in BARS.iter().enumerate() {
        let left = index * width / BARS.len();
        let right = (index + 1) * width / BARS.len();
        canvas.fill_rect(
            left,
            band_top,
            right - left,
            band_bottom - band_top,
            rgb(*red, *green, *blue),
        );
    }
}

/// D3: bitmap text on a plain background.
fn draw_text_scene(canvas: &mut Canvas, width: usize, height: usize) {
    canvas.fill_rect(0, 0, width, height, rgb(0x00, 0x00, 0x40));
    canvas.draw_text("RSTINY GUI", 16, 16, rgb(0xFF, 0xFF, 0xFF));
    canvas.draw_text("HELLO FROM THE FRAMEBUFFER", 16, 32, rgb(0x00, 0xFF, 0x00));
    canvas.draw_text("0123456789 !?:-/", 16, 48, rgb(0xFF, 0xFF, 0x00));
    canvas.draw_text("LEASING WORKS", 16, 64, rgb(0xFF, 0x40, 0x40));
    let _ = width;
    let _ = height;
}

/// D3: full-screen text, then two terminal-style scrolls.
fn draw_scroll_scene(canvas: &mut Canvas, width: usize, height: usize) {
    let background = rgb(0x00, 0x00, 0x00);
    canvas.fill_rect(0, 0, width, height, background);
    for line in 0..20 {
        let mut label = [b' '; 7];
        label[..5].copy_from_slice(b"LINE ");
        label[5] = b'0' + (line / 10) as u8;
        label[6] = b'0' + (line % 10) as u8;
        let text = core::str::from_utf8(&label).expect("ascii");
        canvas.draw_text(text, 8, 8 + line * 16, rgb(0xFF, 0xFF, 0xFF));
    }
    canvas.scroll_up(64, background);
    canvas.scroll_up(64, background);
    canvas.draw_text("SCROLLED", 8, 16, rgb(0xFF, 0xFF, 0x00));
}

/// D4: static screen, then log key presses as they arrive. Exits 0 after
/// `expected` presses, 5 if the keyboard stays quiet.
fn draw_keys_scene(
    service: &Service,
    canvas: &mut Canvas,
    gpu_ep: u64,
    width: usize,
    height: usize,
    expected: usize,
) {
    canvas.fill_rect(0, 0, width, height, rgb(0x00, 0x20, 0x00));
    canvas.draw_text("PRESS KEYS", 16, 16, rgb(0xFF, 0xFF, 0xFF));
    let _ = ipc::call(gpu_ep, gpu::FLUSH, &[0, 0, width as u64, height as u64]);
    let mut received = 0;
    for _ in 0..KEY_POLLS_MAX {
        let Some(reply) = ipc::call(gpu_ep, gpu::INPUT_READ, &[])
            .ok()
            .filter(|reply| reply.label == status::OK)
        else {
            continue;
        };
        if reply.word(0) == 0 {
            let _ = rstiny::sleep(KEY_POLL_MS);
            continue;
        }
        // The reply packs present | type<<40 | code<<24 | value in one word.
        let event_type = (reply.word(0) >> 40) & 0xFFFF;
        let code = (reply.word(0) >> 24) & 0xFFFF;
        let value = reply.word(0) & 0xFF_FFFF;
        if event_type != 1 || value != 1 {
            continue; // not a key press
        }
        match key_char(code as u8) {
            Some(character) => logln!(service, "[gui] key: {character}"),
            None => logln!(service, "[gui] key: <{}>", code),
        }
        received += 1;
        if received >= expected {
            return;
        }
    }
    logln!(service, "[gui] keys timed out");
    service.exit(5);
}

/// Linux keycode → printable character, for the keys the acceptance sends.
/// The QWERTY rows are contiguous runs in Linux keycodes *except* the bottom
/// row (z x c v b n m), which needs its own table.
fn key_char(code: u8) -> Option<char> {
    match code {
        2..=10 => Some((b'1' + code - 2) as char),
        11 => Some('0'),
        16..=25 => Some((b'q' + code - 16) as char),
        30..=38 => Some((b'a' + code - 30) as char),
        44..=50 => Some(b"zxcvbnm"[(code - 44) as usize] as char),
        57 => Some(' '),
        _ => None,
    }
}
