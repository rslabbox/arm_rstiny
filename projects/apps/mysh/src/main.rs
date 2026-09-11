#![no_std]
#![no_main]
//! mysh: a minimal interactive shell. It reads command lines from the console
//! service (polling for RX), lists the FAT32 disk through the fs service, runs
//! commands (`ls`, `cat <file>`, `./hello`, `help`, `exit`) and executes
//! `./hello` by loading the ELF from disk and supervising it with the same
//! loader appmgr uses.
//!
//! The console service is polling-only (no RX interrupt yet), so `read_line`
//! sleeps between polls while it waits for a keystroke.

extern crate alloc;

use alloc::vec::Vec;
use core::hint::spin_loop;

use rstiny::Error;
use rstiny::capability::{
    CNode, CPtr, INIT_ASID_POOL, INIT_CNODE, INIT_UNTYPED, INIT_VSPACE, ObjectType, Page,
    PageTable, RIGHTS_ALL, RIGHTS_READ, RIGHTS_WRITE, Untyped, VM_CACHEABLE, VM_EXECUTE_NEVER,
};
use rstiny::elf::{ChildCap, LOADER_SLOT_BASE, Supervision};
use rstiny::ipc::{self, ReceiveSpec};
use rstiny_protocol::{Argument, SpawnInfo, console, control, fs, status};
use rstiny_runtime::entry;
use rstiny_server::{Service, logln};

/// Address-space windows owned by this task (its ELF/stack live lower).
const FS_BUF_VA: usize = 0x0400_0000; // fs shared buffer, granted on BIND
const SCRATCH_VA: usize = 0x07E0_0000; // loader alias while spawning a child
const TABLE_SLOT: u64 = 44; // L3 covering FS_BUF_VA
const FS_RECV_SLOT: u64 = 60; // landing slot for the fs BIND cap transfer
const CHILD_BUDGET_SLOT: u64 = 90; // sub-Untyped carved for a child
const CHILD_BUDGET_BITS: u64 = 20; // 1 MiB per child
const MAX_FILE: usize = 256 * 1024;

/// Child CSpace layout, matching init/appmgr's convention.
const CHILD_CONTROL: u64 = 140;
const CHILD_CONSOLE: u64 = 51;
const CHILD_BUDGET: u64 = 32;

const PROMPT: &[u8] = b"[rstiny ~]$: ";
const LINE_MAX: usize = 128;

/// Bump allocator over a fixed BSS pool: file images and script text live here
/// and are freed only when the supervisor tears the task down.
const POOL_BYTES: usize = 512 * 1024;
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

#[entry]
fn main(argument: Argument) -> ! {
    let Some(service) = Service::init(argument) else {
        loop {
            spin_loop();
        }
    };
    let fs_ep = service.extra[SpawnInfo::DEP_EP_BASE];
    if fs_ep == 0 {
        logln!(service, "[mysh] no fs dependency");
        service.exit(2);
    }
    if let Err(error) = bind_fs(fs_ep) {
        logln!(service, "[mysh] fs bind failed: {:?}", error);
        service.exit(3);
    }
    logln!(service, "[mysh] ready");
    repl(&service, fs_ep)
}

/// Map the fs shared buffer and bind the fs service.
fn bind_fs(fs_ep: u64) -> Result<(), Error> {
    let cnode = CNode(CPtr(INIT_CNODE));
    Untyped(CPtr(INIT_UNTYPED)).retype(ObjectType::PageTable, 0, cnode.0, TABLE_SLOT, 1)?;
    PageTable(CPtr(TABLE_SLOT)).map(CPtr(INIT_VSPACE), FS_BUF_VA & !0x1F_FFFF)?;
    ipc::set_receive_spec(ReceiveSpec {
        cnode: INIT_CNODE,
        index: FS_RECV_SLOT,
        depth: 64,
    })?;
    let reply = ipc::call_cap(fs_ep, fs::BIND, &[fs::PROTOCOL_VERSION], &[])?;
    if reply.label != status::OK || reply.word(0) != fs::PROTOCOL_VERSION {
        return Err(Error::FailedLookup);
    }
    // SAFETY: the received frame is exclusively ours; nothing else maps it.
    unsafe {
        Page(CPtr(FS_RECV_SLOT)).map(
            CPtr(INIT_VSPACE),
            FS_BUF_VA,
            RIGHTS_READ | RIGHTS_WRITE,
            VM_CACHEABLE | VM_EXECUTE_NEVER,
        )?;
    }
    Ok(())
}

fn repl(service: &Service, fs_ep: u64) -> ! {
    let mut line = [0u8; LINE_MAX];
    loop {
        service.log_bytes(PROMPT);
        let Some(used) = read_line(service, &mut line) else {
            // Ctrl-D: end of input.
            service.log_bytes(b"\n");
            break;
        };
        let text = core::str::from_utf8(&line[..used]).unwrap_or("");
        if !execute(service, fs_ep, text) {
            break;
        }
    }
    logln!(service, "[mysh] bye");
    service.exit(0)
}

/// Poll one byte from the console; `None` when the input FIFO is empty.
fn console_read(console_ep: u64) -> Option<u8> {
    let received = ipc::call(console_ep, console::READ, &[]).ok()?;
    (received.label == status::OK && received.word(0) == 1).then(|| received.word(1) as u8)
}

/// Block until Enter, echoing input. Handles backspace (0x7f/0x08), Ctrl-C
/// (abort the line) and Ctrl-D (EOF). Returns the line length without the
/// newline, or `None` on EOF.
fn read_line(service: &Service, line: &mut [u8]) -> Option<usize> {
    let mut used = 0usize;
    loop {
        match console_read(service.console_ep) {
            Some(b'\r') | Some(b'\n') => {
                service.log_bytes(b"\n");
                return Some(used);
            }
            Some(0x7f) | Some(0x08) => {
                if used > 0 {
                    used -= 1;
                    service.log_bytes(b"\x08 \x08");
                }
            }
            Some(0x03) => {
                service.log_bytes(b"^C\n");
                return Some(0);
            }
            Some(0x04) => return None,
            Some(byte) if (0x20..=0x7e).contains(&byte) => {
                if used < line.len() {
                    line[used] = byte;
                    used += 1;
                    service.log_bytes(&[byte]);
                }
            }
            Some(_) => {}
            None => {
                // No RX interrupt yet: poll without spinning the CPU.
                let _ = rstiny::sleep(5);
            }
        }
    }
}

/// Run one command line. Returns false to leave the shell.
fn execute(service: &Service, fs_ep: u64, line: &str) -> bool {
    let mut parts = line.split_whitespace();
    let Some(command) = parts.next() else {
        return true;
    };
    match command {
        "help" => {
            logln!(
                service,
                "[mysh] commands: ls, cat <file>, ./hello, help, exit"
            );
        }
        "exit" => return false,
        "ls" => list(service, fs_ep),
        "cat" => match parts.next() {
            Some(name) => cat(service, fs_ep, name.as_bytes()),
            None => logln!(service, "[mysh] cat: missing file name"),
        },
        "hello" | "./hello" => run_hello(service, fs_ep),
        other => logln!(service, "[mysh] unknown command: {}", other),
    }
    true
}

/// `ls`: list the FAT32 root directory, batched through the shared buffer.
fn list(service: &Service, fs_ep: u64) {
    let mut start = 0u64;
    loop {
        let Some(received) = fs_call(fs_ep, fs::READDIR, &[start]) else {
            logln!(service, "[mysh] ls: readdir failed");
            return;
        };
        let count = (received.word(0) as usize).min(fs::DIR_ENTRIES_PER_PAGE);
        let next = received.word(1);
        // SAFETY: the fs shared buffer is exclusively mapped by this task and
        // only written while this task is blocked in the READDIR call.
        let records =
            unsafe { core::slice::from_raw_parts(FS_BUF_VA as *const fs::DirEntry, count) };
        for record in records {
            let name = short_name(record);
            if record.is_dir != 0 {
                logln!(service, "  {}/", name);
            } else {
                logln!(service, "  {}  {} bytes", name, record.size);
            }
        }
        if next == 0 {
            break;
        }
        start = next;
    }
}

/// `cat <file>`: stream a file's bytes to the console.
fn cat(service: &Service, fs_ep: u64, name: &[u8]) {
    let Some((file_id, size)) = fs_open(fs_ep, name) else {
        logln!(service, "[mysh] cat: not found");
        return;
    };
    let mut offset = 0u64;
    while offset < size {
        let length = (size - offset).min(0x1000);
        let Some(read) = fs_read(fs_ep, file_id, offset, length) else {
            break;
        };
        if read == 0 {
            break;
        }
        // SAFETY: the shared buffer is exclusively mapped, filled by the fs
        // service before its reply arrived.
        let chunk = unsafe { core::slice::from_raw_parts(FS_BUF_VA as *const u8, read) };
        service.log_bytes(chunk);
        offset += read as u64;
    }
    let _ = ipc::call(fs_ep, fs::CLOSE, &[file_id]);
}

/// `./hello`: load `HELLO.ELF` from disk and run it as a supervised child.
fn run_hello(service: &Service, fs_ep: u64) {
    let Some(self_ep) = service
        .extra
        .get(SpawnInfo::SELF_EP)
        .copied()
        .filter(|slot| *slot != 0)
    else {
        logln!(service, "[mysh] hello: no self endpoint");
        return;
    };
    let Some(image) = read_file(fs_ep, b"HELLO.ELF") else {
        logln!(service, "[mysh] hello: cannot read HELLO.ELF");
        return;
    };
    let cnode = CNode(CPtr(INIT_CNODE));
    // One fresh sub-Untyped per spawn; Task::destroy revokes it.
    if Untyped(CPtr(INIT_UNTYPED))
        .retype(
            ObjectType::Untyped,
            CHILD_BUDGET_BITS,
            cnode.0,
            CHILD_BUDGET_SLOT,
            1,
        )
        .is_err()
    {
        logln!(service, "[mysh] hello: cannot budget the child");
        return;
    }
    let child_info = SpawnInfo {
        magic: SpawnInfo::MAGIC,
        version: SpawnInfo::VERSION,
        control_ep: CHILD_CONTROL,
        command_ep: 0,
        untyped: CHILD_BUDGET,
        rom_start: 0,
        rom_count: 0,
        extra: {
            let mut extra = [0; SpawnInfo::EXTRA_LEN];
            extra[SpawnInfo::CONSOLE_EP] = CHILD_CONSOLE;
            extra
        },
    };
    // SAFETY: a fully initialized repr(C) value with no padding.
    let info = unsafe {
        core::slice::from_raw_parts(
            &child_info as *const SpawnInfo as *const u8,
            core::mem::size_of::<SpawnInfo>(),
        )
    };
    let caps = [
        ChildCap {
            slot: CHILD_CONTROL,
            source: self_ep,
            rights: RIGHTS_ALL,
            badge: 1,
        },
        ChildCap {
            slot: CHILD_CONSOLE,
            source: service.console_ep,
            rights: RIGHTS_ALL,
            badge: 0,
        },
        ChildCap {
            slot: CHILD_BUDGET,
            source: CHILD_BUDGET_SLOT,
            rights: RIGHTS_ALL,
            badge: 0,
        },
        ChildCap {
            slot: 6,
            source: INIT_ASID_POOL,
            rights: RIGHTS_ALL,
            badge: 0,
        },
    ];
    // SAFETY: SCRATCH_VA is an unmapped page this task reserves exclusively.
    logln!(service, "[mysh] ./hello ({} bytes)", image.len());
    let spawned = unsafe {
        rstiny::elf::spawn_supervised(
            &image,
            SCRATCH_VA,
            CHILD_BUDGET_SLOT,
            &Supervision {
                info,
                fault_ep: CHILD_CONTROL,
                caps: &caps,
                slot_base: LOADER_SLOT_BASE,
            },
        )
    };
    match spawned {
        Ok(task) => {
            // hello is a service: it announces READY and later EXIT on the
            // control endpoint (our own self endpoint, badged). No other task
            // shares this endpoint, so a flat recv loop supervises it.
            let code = loop {
                let Ok(received) = ipc::recv(self_ep) else {
                    continue;
                };
                match received.label {
                    control::READY | control::REPORT => {
                        let _ = ipc::reply(0, &[]);
                    }
                    control::EXIT => {
                        let _ = ipc::reply(0, &[]);
                        break received.word(0);
                    }
                    // A kernel fault is delivered on the same endpoint once the
                    // receiving task is blocked; treat it as a failed run.
                    _ => break u64::MAX,
                }
            };
            let _ = task.destroy();
            logln!(service, "[mysh] hello exited: {}", code);
        }
        Err(error) => {
            logln!(service, "[mysh] hello spawn failed: {:?}", error);
        }
    }
}

// ---- fs client helpers -----------------------------------------------------

fn fs_call(fs_ep: u64, label: u64, words: &[u64]) -> Option<ipc::Received> {
    let received = ipc::call(fs_ep, label, words).ok()?;
    (received.label == status::OK).then_some(received)
}

fn fs_open(fs_ep: u64, name: &[u8]) -> Option<(u64, u64)> {
    if name.is_empty() || name.len() > 12 {
        return None;
    }
    let mut words = [0u64; 3];
    words[0] = name.len() as u64;
    for (index, byte) in name.iter().enumerate() {
        words[1 + index / 8] |= u64::from(*byte) << (8 * (index % 8));
    }
    let received = fs_call(fs_ep, fs::OPEN, &words)?;
    Some((received.word(0), received.word(1)))
}

fn fs_read(fs_ep: u64, file_id: u64, offset: u64, length: u64) -> Option<usize> {
    let received = fs_call(fs_ep, fs::READ, &[file_id, offset, length])?;
    Some(received.word(0) as usize)
}

fn read_file(fs_ep: u64, name: &[u8]) -> Option<Vec<u8>> {
    let (file_id, size) = fs_open(fs_ep, name)?;
    if size > MAX_FILE as u64 {
        return None;
    }
    let mut data = Vec::new();
    let mut offset = 0u64;
    while offset < size {
        let length = (size - offset).min(0x1000);
        let read = fs_read(fs_ep, file_id, offset, length)?;
        if read == 0 {
            break;
        }
        // SAFETY: shared buffer, filled before the reply; see `cat`.
        let chunk = unsafe { core::slice::from_raw_parts(FS_BUF_VA as *const u8, read) };
        data.extend_from_slice(chunk);
        offset += read as u64;
    }
    let _ = ipc::call(fs_ep, fs::CLOSE, &[file_id]);
    Some(data)
}

fn short_name(entry: &fs::DirEntry) -> &str {
    let end = entry
        .name
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(entry.name.len());
    core::str::from_utf8(&entry.name[..end]).unwrap_or("?")
}
