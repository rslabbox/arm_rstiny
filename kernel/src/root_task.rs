//! Bootstrap one root task. No capability creation, general scheduler or IPC yet.
use crate::{
    arch::{TrapFrame, user},
    config::MemFlags,
};
use core::ptr::{addr_of, addr_of_mut};
use kernel_abi::*;

const IMAGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/root.boot"));

#[derive(Clone, Copy)]
#[repr(u64)]
enum State {
    Inactive,
    Running,
    Suspended,
    Faulted,
}
#[repr(C)]
struct RootTask {
    state: State,
    vspace: u64,
    context: TrapFrame,
}
#[unsafe(no_mangle)]
static mut ROOT_TASK: RootTask = RootTask {
    state: State::Inactive,
    vspace: 0,
    context: TrapFrame {
        r: [0; 31],
        usp: 0,
        elr: 0,
        spsr: 0,
    },
};

fn word(offset: usize) -> u64 {
    u64::from_le_bytes(
        IMAGE
            .get(offset..offset + 8)
            .expect("truncated boot module")
            .try_into()
            .unwrap(),
    )
}

#[inline(never)]
#[unsafe(no_mangle)]
pub extern "C" fn start_root() -> ! {
    assert!(
        IMAGE.len() >= 24 && &IMAGE[..8] == b"RSTROOT\0",
        "missing/invalid root image; build with make build"
    );
    let entry = word(8);
    let count = word(16) as usize;
    assert!((1..=32).contains(&count) && IMAGE.len() >= 24 + count * 40);
    let vspace = unsafe { user::init() };
    let mut occupied = [false; 256];
    let mut valid_entry = false;
    let mut image_end = IMAGE_START;
    for i in 0..count {
        let offset = 24 + i * 40;
        let va = word(offset);
        let size = word(offset + 8);
        let file_size = word(offset + 16);
        let file_offset = word(offset + 24);
        let flags = word(offset + 32);
        assert!(va >= IMAGE_START && size > 0 && va < IMAGE_END && size <= IMAGE_END - va);
        assert!(va.is_multiple_of(PAGE_SIZE) && file_size <= size && matches!(flags, 4..=6));
        assert!(file_offset >= (24 + count * 40) as u64 && file_offset <= IMAGE.len() as u64);
        assert!(file_size <= IMAGE.len() as u64 - file_offset);
        valid_entry |= flags == 5 && (va..va + file_size).contains(&entry);
        image_end = image_end.max((va + size).next_multiple_of(PAGE_SIZE));
        for page_offset in (0..size).step_by(PAGE_SIZE as usize) {
            let page_va = va + page_offset;
            let index = ((page_va - IMAGE_START) / PAGE_SIZE) as usize;
            assert!(!occupied[index], "overlapping user segments");
            occupied[index] = true;
            let mut rights = MemFlags::READ;
            if flags == 5 {
                rights |= MemFlags::EXECUTE;
            }
            if flags == 6 {
                rights |= MemFlags::WRITE;
            }
            let page = unsafe { user::map_page(page_va, rights) };
            let bytes = file_size.saturating_sub(page_offset).min(PAGE_SIZE) as usize;
            if bytes != 0 {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        IMAGE.as_ptr().add((file_offset + page_offset) as usize),
                        page,
                        bytes,
                    )
                };
            }
        }
    }
    assert!(valid_entry, "root entry outside executable segment");
    unsafe {
        for va in (STACK_START..STACK_END).step_by(PAGE_SIZE as usize) {
            user::map_page(va, MemFlags::READ | MemFlags::WRITE);
        }
        user::map_page(IPC_BUFFER_VA, MemFlags::READ | MemFlags::WRITE);
        let bootinfo = user::map_page(BOOTINFO_VA, MemFlags::READ).cast::<BootInfo>();
        bootinfo.write(BootInfo {
            magic: BOOTINFO_MAGIC,
            version: ABI_VERSION,
            size: core::mem::size_of::<BootInfo>() as u64,
            page_size: PAGE_SIZE,
            features: if log::max_level() != log::LevelFilter::Off {
                FEATURE_DEBUG_CONSOLE
            } else {
                0
            },
            ipc_buffer: IPC_BUFFER_VA,
            image_start: IMAGE_START,
            image_end,
            stack_start: STACK_START,
            stack_end: STACK_END,
        });
        let mut frame = TrapFrame {
            usp: STACK_END,
            elr: entry,
            spsr: 0x3c0,
            ..TrapFrame::default()
        };
        frame.r[0] = BOOTINFO_VA;
        addr_of_mut!(ROOT_TASK).write(RootTask {
            state: State::Running,
            vspace,
            context: frame,
        });
        log::info!(
            "Starting fatboot: entry={:#x}, BootInfo={:#x}, EL0",
            entry,
            BOOTINFO_VA
        );
        user::activate(vspace);
        user::enter(addr_of!(ROOT_TASK.context))
    }
}

pub fn syscall(frame: &mut TrapFrame) {
    frame.r[0] = match frame.r[8] {
        SYS_YIELD => OK, // The sole runnable thread continues.
        SYS_DEBUG_PUTCHAR if frame.r[0] > 255 => INVALID_ARGUMENT,
        SYS_DEBUG_PUTCHAR if log::max_level() == log::LevelFilter::Off => UNSUPPORTED,
        SYS_DEBUG_PUTCHAR => {
            crate::utils::logging::debug_putchar(frame.r[0] as u8);
            OK
        }
        SYS_SUSPEND_SELF => stop(frame, false),
        _ => UNSUPPORTED,
    };
}

pub fn stop(frame: &TrapFrame, fault: bool) -> ! {
    unsafe {
        addr_of_mut!(ROOT_TASK.context).write(*frame);
        addr_of_mut!(ROOT_TASK.state).write(if fault {
            State::Faulted
        } else {
            State::Suspended
        });
    }
    log::info!("fatboot {}", if fault { "faulted" } else { "suspended" });
    root_idle()
}

#[unsafe(naked)]
#[unsafe(no_mangle)]
extern "C" fn root_idle() -> ! {
    core::arch::naked_asm!("msr daifset, #0xf", "2:", "wfe", "b 2b");
}
