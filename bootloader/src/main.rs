#![no_std]
#![no_main]

mod archive;
mod elf;
mod pl011;

use core::{
    arch::{asm, naked_asm},
    fmt::{self, Write},
    ptr::addr_of,
};
use elf::{Elf, OFFSET, PAGE, page_up};

#[allow(dead_code)]
mod platform {
    include!(concat!(env!("OUT_DIR"), "/platform.rs"));
}
include!(concat!(env!("OUT_DIR"), "/archive.rs"));

unsafe extern "C" {
    static __archive_start: u8;
    static __archive_end: u8;
    static __bss_start: u8;
    static __bss_end: u8;
}

#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.boot")]
unsafe extern "C" fn _start() -> ! {
    naked_asm!(
        "msr daifset, #0xf",
        "mrs x9, mpidr_el1", "ldr x10, =0xff00ffffff", "tst x9, x10", "b.ne 2f",
        "mrs x9, CurrentEL", "cmp x9, #4", "b.ne 2f",
        "mrs x9, sctlr_el1", "ldr x10, =0x1005", "tst x9, x10", "b.ne 2f",
        "msr spsel, #1", "ldr x9, =__stack_top", "mov sp, x9",
        "b {main}", "2:", "wfe", "b 2b", main = sym start,
    );
}

extern "C" fn start() -> ! {
    // QEMU loads ELF NOBITS as zero too, but startup owns BSS initialization.
    unsafe {
        core::ptr::write_bytes(
            addr_of!(__bss_start).cast_mut(),
            0,
            addr_of!(__bss_end) as usize - addr_of!(__bss_start) as usize,
        );
    }
    uart().init();
    let _ = writeln!(Console, "Rust bootloader started (AArch64 EL1)");
    match prepare() {
        Ok(info) => unsafe { enter(info) },
        Err(message) => fail(message),
    }
}

struct Handoff {
    image_start: usize,
    image_end: usize,
    offset: usize,
    root_entry: usize,
    dtb: usize,
    dtb_size: usize,
    kernel_entry: usize,
}

fn prepare() -> Result<Handoff, &'static str> {
    let archive = unsafe {
        core::slice::from_raw_parts(
            addr_of!(__archive_start),
            addr_of!(__archive_end) as usize - addr_of!(__archive_start) as usize,
        )
    };
    let [kernel_bytes, dtb, root_bytes] = archive::files(archive)?;
    let kernel = Elf::parse(kernel_bytes)?;
    let root = Elf::parse(root_bytes)?;
    if platform::RAM_START != 0x4000_0000 || platform::RAM_END != 0x4800_0000 {
        return Err("unsupported platform RAM");
    }
    if kernel.start != OFFSET + 0x4020_0000
        || kernel
            .segments()
            .iter()
            .any(|segment| segment.va.checked_sub(OFFSET) != Some(segment.pa))
    {
        return Err("invalid kernel physical layout");
    }
    if root.start != 0x0040_0000
        || root.end != 0x0060_0000
        || !root.segments().iter().any(|segment| {
            segment.flags == 6
                && segment.va <= 0x005f_c000
                && segment.va + segment.memsz == 0x0060_0000
        })
    {
        return Err("invalid root image or runtime stack");
    }
    if dtb.len() < 40 || dtb[..4] != [0xd0, 0x0d, 0xfe, 0xed] {
        return Err("invalid DTB header");
    }
    let dtb_size = u32::from_be_bytes(dtb[4..8].try_into().unwrap()) as usize;
    if !(40..=1024 * 1024).contains(&dtb_size) || dtb_size > dtb.len() {
        return Err("invalid DTB total size");
    }
    let dtb_address = kernel
        .end
        .checked_sub(OFFSET)
        .ok_or("kernel end overflow")?;
    let image_start = page_up(
        dtb_address
            .checked_add(dtb_size)
            .ok_or("DTB placement overflow")?,
    )?;
    let image_end = image_start
        .checked_add(root.end - root.start)
        .ok_or("root placement overflow")?;
    let reserved_end = image_end
        .checked_add(PAGE)
        .ok_or("headers placement overflow")?;
    // One contiguous destination region below 0x42000000: kernel, DTB,
    // root and retained headers. Loader, stack, tables and archive are all
    // >=0x44000000, so no destination can overwrite the reader itself.
    if dtb_address <= 0x4020_0000 || reserved_end > 0x4200_0000 {
        return Err("boot images exceed kernel RAM window");
    }
    let _ = writeln!(
        Console,
        "kernel entry={:#x}; root paddr={:#x}..{:#x}",
        kernel.entry, image_start, image_end
    );
    // All input metadata and all ranges have been validated before this point.
    unsafe {
        kernel.load(0x4020_0000);
        core::ptr::copy_nonoverlapping(dtb.as_ptr(), dtb_address as *mut u8, dtb_size);
        root.load(image_start);
        core::ptr::write_bytes(image_end as *mut u8, 0, PAGE);
        (image_end as *mut u32).write(root.count as u32);
        ((image_end + 4) as *mut u32).write(56);
        core::ptr::copy_nonoverlapping(
            root.headers.as_ptr(),
            (image_end + 8) as *mut u8,
            root.headers.len(),
        );
    }
    Ok(Handoff {
        image_start,
        image_end,
        offset: image_start - root.start,
        root_entry: root.entry,
        dtb: dtb_address,
        dtb_size,
        kernel_entry: kernel.entry,
    })
}

#[repr(C, align(4096))]
struct Table([u64; 512]);
static mut ROOT: Table = Table([0; 512]);
static mut LEVEL1: Table = Table([0; 512]);

unsafe fn enter(info: Handoff) -> ! {
    unsafe {
        let l1 = core::ptr::addr_of_mut!(LEVEL1).cast::<u64>();
        // A temporary identity map and its high alias share one tree. Only
        // privileged accesses are permitted. The kernel replaces both roots.
        l1.add(0).write(1 | (1 << 10) | (1 << 53) | (1 << 54));
        l1.add(1)
            .write(0x4000_0000 | 1 | (4 << 2) | (3 << 8) | (1 << 10) | (1 << 54));
        core::ptr::addr_of_mut!(ROOT)
            .cast::<u64>()
            .write(l1 as u64 | 3);
        let mmfr: u64;
        asm!("mrs {value}, id_aa64mmfr0_el1", value=out(reg) mmfr, options(nomem, nostack));
        let tcr = 16u64
            | (16 << 16)
            | (1 << 8)
            | (1 << 10)
            | (3 << 12)
            | (1 << 24)
            | (1 << 26)
            | (3 << 28)
            | (2 << 30)
            | ((mmfr & 7) << 32)
            | (1 << 36);
        asm!(
            "dsb sy", "ic iallu", "dsb sy", "isb",
            "msr mair_el1, {mair}", "msr tcr_el1, {tcr}",
            "msr ttbr0_el1, {root}", "msr ttbr1_el1, {root}", "isb",
            "tlbi vmalle1", "dsb sy", "isb",
            "mrs x9, sctlr_el1", "orr x9, x9, #1", "orr x9, x9, #4",
            "orr x9, x9, #0x1000", "msr sctlr_el1, x9", "isb",
            mair=in(reg) 0x0000_aaff_440c_0400u64, tcr=in(reg) tcr,
            root=in(reg) addr_of!(ROOT) as usize, out("x9") _, options(nostack),
        );
        let _ = write!(Console, "Enabling MMU and jumping to entry point...\n\n");
        asm!("br x6", in("x0") info.image_start, in("x1") info.image_end,
            in("x2") info.offset, in("x3") info.root_entry, in("x4") info.dtb,
            in("x5") info.dtb_size, in("x6") info.kernel_entry, options(noreturn));
    }
}

fn uart() -> pl011::Pl011Uart {
    // SAFETY: one CPU, IRQs masked; this physical UART address is accessible
    // before the MMU and remains mapped Device through the temporary tree.
    unsafe { pl011::Pl011Uart::new(platform::UART_BASE as *mut u8) }
}
struct Console;
impl Write for Console {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            if byte == b'\n' {
                put_byte(b'\r')?;
            }
            put_byte(byte)?;
        }
        Ok(())
    }
}
fn put_byte(byte: u8) -> fmt::Result {
    uart().putchar(byte)
}
fn fail(message: &str) -> ! {
    let _ = writeln!(Console, "bootloader: error: {message}");
    bootloader_halt()
}
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn bootloader_halt() -> ! {
    loop {
        unsafe {
            asm!("wfe", options(nomem, nostack));
        }
    }
}
#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    let _ = writeln!(Console, "bootloader: error: {info}");
    bootloader_halt()
}
