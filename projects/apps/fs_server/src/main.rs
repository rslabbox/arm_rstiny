#![no_std]
#![no_main]
//! fs_server: a read-only FAT32 service on top of the block protocol
//! (docs/disk-driver.md section 7), built on the maintained `hadris-fat`
//! crate. One bound client at a time; file data is served through a
//! server-owned shared buffer frame granted on BIND.

extern crate alloc;

use alloc::vec::Vec;
use core::hint::spin_loop;

use embedded_io::{ErrorKind, Read as IoRead, Seek as IoSeek, SeekFrom};
use hadris_fat::sync::{FatVolume, FatVolumeReadExt, FileEntry};
use rstiny::capability::{
    CNode, CPtr, INIT_CNODE, INIT_UNTYPED, INIT_VSPACE, ObjectType, Page, PageTable, RIGHTS_READ,
    RIGHTS_WRITE, Untyped, VM_CACHEABLE, VM_EXECUTE_NEVER,
};
use rstiny::ipc;
use rstiny_protocol::{Argument, SpawnInfo, block, control, fs, status};
use rstiny_runtime::entry;
use rstiny_server::{Service, logln};

// One 2 MiB window with a single L3 from the budget covers the block device's
// shared buffer and the buffer this service grants to its own client.
const WINDOW_VA: usize = 0x0400_0000;
const CLIENT_BUF_VA: usize = WINDOW_VA; // block's shared buffer (received)
const SHARE_VA: usize = WINDOW_VA + 0x1000; // frame granted to fs clients
const TABLE_SLOT: u64 = 44;
const RECV_SLOT: u64 = 60; // landing slot for block's BIND cap transfer
const SHARE_SLOT: u64 = 61; // frame granted to the fs client on BIND
const MAX_FILES: usize = 4;
const SECTOR_SIZE: u64 = 512;
const SECTORS_PER_READ: u64 = 8;

/// Bump allocator over a fixed BSS pool: hadris-fat path names need owned
/// strings, and nothing is freed before the supervisor tears the task down.
const POOL_BYTES: usize = 64 * 1024;
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

/// The block device seen through the shared buffer: reads travel over IPC and
/// land in `CLIENT_BUF_VA`, then this adapter copies the requested window into
/// the filesystem library's buffer.
struct BlockDevice {
    ep: u64,
    position: u64,
    disk_bytes: u64,
}
impl BlockDevice {
    fn fetch(&mut self, lba: u64, count: u64) -> Result<(), ErrorKind> {
        let Ok(received) = ipc::call(self.ep, block::READ, &[lba, count]) else {
            return Err(ErrorKind::Other);
        };
        if received.label != status::OK {
            return Err(ErrorKind::Other);
        }
        Ok(())
    }
}
impl embedded_io::ErrorType for BlockDevice {
    type Error = ErrorKind;
}
impl IoRead for BlockDevice {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        let mut filled = 0;
        while filled < buf.len() && (self.position as u64) < self.disk_bytes {
            let lba = self.position / SECTOR_SIZE;
            let remaining_sectors = (self.disk_bytes - self.position).div_ceil(SECTOR_SIZE);
            let count = remaining_sectors.min(SECTORS_PER_READ);
            self.fetch(lba, count)?;
            let window = lba * SECTOR_SIZE;
            let offset = (self.position - window) as usize;
            let available = (count * SECTOR_SIZE) as usize - offset;
            let take = available.min(buf.len() - filled);
            // SAFETY: CLIENT_BUF_VA is exclusively mapped; the block service
            // only writes it while this task is blocked in the READ call.
            let source =
                unsafe { core::slice::from_raw_parts((CLIENT_BUF_VA + offset) as *const u8, take) };
            buf[filled..filled + take].copy_from_slice(source);
            self.position += take as u64;
            filled += take;
        }
        Ok(filled)
    }
}
impl IoSeek for BlockDevice {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64, Self::Error> {
        let target = match pos {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::End(delta) => (self.disk_bytes as i64)
                .checked_add(delta)
                .map(|v| u64::try_from(v).ok())
                .flatten(),
            SeekFrom::Current(delta) => (self.position as i64)
                .checked_add(delta)
                .map(|v| u64::try_from(v).ok())
                .flatten(),
        };
        match target {
            Some(value) => {
                self.position = value;
                Ok(self.position)
            }
            None => Err(ErrorKind::InvalidInput),
        }
    }
}

/// Extract a file name packed into the message registers (8 bytes per MR).
fn name_from_words(received: &rstiny::ipc::Received) -> Option<Vec<u8>> {
    let length = received.word(0) as usize;
    if length == 0 || length > 12 + 1 || length > 2 * 8 {
        return None;
    }
    let mut name = Vec::with_capacity(length);
    for index in 0..length {
        name.push((received.word(1 + index / 8) >> (8 * (index % 8))) as u8);
    }
    Some(name)
}

#[entry]
fn main(argument: Argument) -> ! {
    let Some(service) = Service::init(argument) else {
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
    let Some(dep_count) = service
        .extra
        .get(SpawnInfo::DEP_COUNT)
        .copied()
        .filter(|c| *c >= 1)
    else {
        service.exit(2);
    };
    let block_ep = service.extra[SpawnInfo::DEP_EP_BASE + 0]; // depends = block
    let _ = dep_count;
    let cnode = CNode(CPtr(INIT_CNODE));
    let vspace = CPtr(INIT_VSPACE);
    // The covering L3 and the client-shared frame come from the budget.
    if Untyped(CPtr(INIT_UNTYPED))
        .retype(ObjectType::PageTable, 0, cnode.0, TABLE_SLOT, 1)
        .is_err()
        || Untyped(CPtr(INIT_UNTYPED))
            .retype(ObjectType::SmallPage, 0, cnode.0, SHARE_SLOT, 1)
            .is_err()
    {
        logln!(service, "[fs] cannot budget the fs structures");
        service.exit(3);
    }
    // SAFETY: mappings exclusive to this task; the shared frames have no
    // cached aliases on either side.
    unsafe {
        if PageTable(CPtr(TABLE_SLOT))
            .map(vspace, WINDOW_VA & !0x1F_FFFF)
            .is_err()
            || Page(CPtr(SHARE_SLOT))
                .map(
                    vspace,
                    SHARE_VA,
                    RIGHTS_READ | RIGHTS_WRITE,
                    VM_CACHEABLE | VM_EXECUTE_NEVER,
                )
                .is_err()
        {
            logln!(service, "[fs] cannot map the fs window");
            service.exit(3);
        }
    }
    // BIND with block: the reply carries the shared DMA frame; publish the
    // receive spec first so the transfer lands in RECV_SLOT.
    if ipc::set_receive_spec(rstiny::ipc::ReceiveSpec {
        cnode: INIT_CNODE,
        index: RECV_SLOT,
        depth: 64,
    })
    .is_err()
    {
        service.exit(3);
    }
    let Ok(reply) = ipc::call_cap(block_ep, block::BIND, &[block::PROTOCOL_VERSION], &[]) else {
        logln!(service, "[fs] block BIND failed");
        service.exit(5);
    };
    // Reply convention: the label carries the status, MRs the payload.
    if reply.label != status::OK || reply.word(0) != block::PROTOCOL_VERSION {
        logln!(service, "[fs] block BIND rejected");
        service.exit(5);
    }
    let capacity = ipc::call(block_ep, block::CAPACITY, &[])
        .map(|received| received.word(0))
        .unwrap_or(0);
    if capacity == 0 {
        logln!(service, "[fs] capacity query failed");
        service.exit(5);
    }
    // Map the received frame (now in RECV_SLOT) as the block data window.
    // SAFETY: exclusively mapped by this task.
    unsafe {
        if Page(CPtr(RECV_SLOT))
            .map(
                vspace,
                CLIENT_BUF_VA,
                RIGHTS_READ | RIGHTS_WRITE,
                VM_CACHEABLE | VM_EXECUTE_NEVER,
            )
            .is_err()
        {
            logln!(service, "[fs] cannot map the block buffer");
            service.exit(3);
        }
    }
    let device = BlockDevice {
        ep: block_ep,
        position: 0,
        disk_bytes: capacity * SECTOR_SIZE,
    };
    let Ok(volume) = FatVolume::open(device) else {
        logln!(service, "[fs] mount failed");
        service.exit(6);
    };
    logln!(service, "[fs] mounted, {} sectors", capacity);

    // Acceptance hook (FAT_TEST=1): open HELLO.ELF, read its first sector and
    // log size plus checksum for the check script to compare with the image.
    if option_env!("FAT_TEST").is_some_and(|value| value == "1") {
        match lookup(&volume, b"HELLO.ELF") {
            Some(entry) => {
                let mut reader = match volume.read_file(&entry) {
                    Ok(reader) => reader,
                    Err(_) => {
                        logln!(service, "[fs] test open failed");
                        service.exit(7);
                    }
                };
                // 4 KiB crosses cluster boundaries, so a looping FAT chain
                // surfaces as a read error instead of a first-cluster success.
                let mut buffer = [0u8; 4096];
                let read = reader.read(&mut buffer).unwrap_or(0);
                let sum: u32 = buffer[..read].iter().copied().map(u32::from).sum();
                logln!(service, "[fs] test size={}", entry.len());
                logln!(service, "[fs] test read={read} sum={sum:#x}");
            }
            None => {
                logln!(service, "[fs] test file not found");
                service.exit(7);
            }
        }
    }

    let mut files: [Option<FileEntry>; MAX_FILES] = core::array::from_fn(|_| None);
    let mut bound: Option<u64> = None;
    loop {
        let Ok(received) = ipc::recv(self_ep) else {
            continue;
        };
        match received.label {
            fs::BIND => {
                let version = received.word(0);
                if version != fs::PROTOCOL_VERSION {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                }
                let reply =
                    ipc::reply_cap(status::OK, &[fs::PROTOCOL_VERSION, 0x1000], &[SHARE_SLOT]);
                if reply.is_ok() {
                    bound = Some(received.badge);
                    logln!(service, "[fs] client 0x{:x} bound", received.badge);
                }
            }
            fs::OPEN => {
                if bound != Some(received.badge) {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                }
                let Some(name) = name_from_words(&received) else {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                };
                let Some(entry) = lookup(&volume, &name) else {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                };
                let Some(slot) = files.iter_mut().position(|slot| slot.is_none()) else {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                };
                files[slot] = Some(entry.clone());
                let _ = ipc::reply(status::OK, &[slot as u64, entry.len()]);
            }
            fs::READ => {
                if bound != Some(received.badge) {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                }
                let file_id = received.word(0) as usize;
                let offset = received.word(1);
                let length = received.word(2) as usize;
                let Some(Some(entry)) = files.get_mut(file_id) else {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                };
                let entry = entry.clone();
                if length == 0 || length > 0x1000 {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                }
                let Ok(mut reader) = volume.read_file(&entry) else {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                };
                if reader.seek(SeekFrom::Start(offset)).is_err() {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                }
                // SAFETY: SHARE_VA is exclusively mapped; the client reads it
                // only after this reply arrives.
                let buffer =
                    unsafe { core::slice::from_raw_parts_mut(SHARE_VA as *mut u8, length) };
                match reader.read(buffer) {
                    Ok(read) => {
                        let _ = ipc::reply(status::OK, &[read as u64]);
                    }
                    Err(_) => {
                        let _ = ipc::reply(status::ERROR, &[0]);
                    }
                }
            }
            fs::CLOSE => {
                let file_id = received.word(0) as usize;
                if let Some(slot) = files.get_mut(file_id) {
                    *slot = None;
                }
                let _ = ipc::reply(status::OK, &[]);
            }
            fs::STAT => {
                let Some(name) = name_from_words(&received) else {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                };
                match lookup(&volume, &name) {
                    Some(entry) => {
                        let _ = ipc::reply(status::OK, &[entry.len(), entry.is_directory() as u64]);
                    }
                    None => {
                        let _ = ipc::reply(status::ERROR, &[0]);
                    }
                }
            }
            control::DEPENDENCY_LOST => {
                // The block service died: our cached volume and buffer die
                // with it. Exit and let init rebuild the chain.
                logln!(service, "[fs] dependency lost; exiting");
                service.exit(9);
            }
            // control::STOP shares label 0x104 with fs::STAT on this
            // endpoint; v1 supervisors reap without a handshake (the
            // DEPENDENCY_LOST path above covers graceful rebuilds).
            _ => {
                let _ = ipc::reply(status::ERROR, &[]);
            }
        }
    }
}

fn lookup(volume: &FatVolume<BlockDevice>, name: &[u8]) -> Option<FileEntry> {
    let dir = volume.root_dir();
    for entry in dir.entries() {
        let Ok(entry) = entry else { continue };
        if entry.name().as_bytes() == name && entry.as_entry().is_some_and(|file| file.is_file()) {
            return Some(entry.as_entry()?.clone());
        }
    }
    None
}
