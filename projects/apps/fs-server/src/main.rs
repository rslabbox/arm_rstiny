#![no_std]
#![no_main]
//! fs-server: a read-only FAT32 service on top of the block protocol
//! (docs/disk-driver.md section 7), built on the maintained `hadris-fat`
//! crate. Protocol v2 (docs/roadmap-next.md P2.1): several concurrently bound
//! clients, each with a private shared-buffer page and its own file-handle
//! table, and long names (up to 255 bytes) carried through the IPC buffer.
//! v1 clients (short 8.3 names) keep working unchanged.

extern crate alloc;

use alloc::{boxed::Box, vec::Vec};


mod backends;
mod blockdev;

use backends::{FileSystem, OpenFile};
use hadris_fat::sync::FatVolume;
use lwext4_rust::FsConfig;
use lwext4_rust::Ext4Filesystem;
use blockdev::{BlockDevice, CLIENT_BUF_VA, SECTOR_SIZE};
use rstiny::capability::{
    CNode, CPtr, INIT_CNODE, INIT_UNTYPED, INIT_VSPACE, ObjectType, Page, PageTable, RIGHTS_READ,
    RIGHTS_WRITE, Untyped, VM_CACHEABLE, VM_EXECUTE_NEVER,
};
use rstiny::ipc;
use rstiny_alloc::Heap;
use rstiny_protocol::{SpawnInfo, block, control, fs, status};
use rstiny_runtime::entry;
use rstiny_server::{Service, logln};

// One 2 MiB window with a single L3 from the budget covers the block device's
// shared buffer (CLIENT_BUF_VA, see blockdev) and one buffer page per fs
// client.
const WINDOW_VA: usize = CLIENT_BUF_VA;
const SHARE_VA: usize = CLIENT_BUF_VA + 0x1000; // first per-client page
const TABLE_SLOT: u64 = 44;
const RECV_SLOT: u64 = 60; // landing slot for block's BIND cap transfer
const SHARE_SLOT: u64 = 61; // first frame granted to an fs client on BIND
const MAX_FILES: usize = 4;
const MAX_CLIENTS: usize = fs::MAX_CLIENTS as usize;

/// One bound client: its badge, its granted shared-buffer page and its own
/// file-handle table, so concurrent clients cannot read each other's handles
/// or clobber each other's buffers. The table lives on the bump heap: with
/// the `lfn` feature a `FileEntry` is ~580 bytes (inline UTF-16 long name),
/// and four clients of those would not fit main's stack frame.
struct Client {
    badge: u64,
    share_va: usize,
    files: alloc::boxed::Box<[Option<OpenFile>; MAX_FILES]>,
}

/// Task heap: rstiny-alloc (interpreter-app.md 决策 B) — the same first-fit
/// allocator the C staticlib exports, shared by every Rust task. Frees are
/// reused, and growth comes from this task's own Untyped budget.
#[global_allocator]
static HEAP: Heap = Heap;

/// Extract a file name packed into the message registers (8 bytes per MR).
/// v2 names reach [`fs::MAX_NAME_LEN`] bytes through the IPC buffer; the
/// kernel checks the receive side, so `word()` sees every message word.
fn name_from_words(received: &rstiny::ipc::Received) -> Option<Vec<u8>> {
    let length = received.word(0) as usize;
    if length == 0 || length > fs::MAX_NAME_LEN + 1 {
        return None;
    }
    let mut name = Vec::with_capacity(length);
    for index in 0..length {
        name.push((received.word(1 + index / 8) >> (8 * (index % 8))) as u8);
    }
    Some(name)
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
    // The covering L3 and one client-shared frame per served client come from
    // the budget.
    if Untyped(CPtr(INIT_UNTYPED))
        .retype(ObjectType::PageTable, 0, cnode.0, TABLE_SLOT, 1)
        .is_err()
        || Untyped(CPtr(INIT_UNTYPED))
            .retype(
                ObjectType::SmallPage,
                0,
                cnode.0,
                SHARE_SLOT,
                MAX_CLIENTS as u64,
            )
            .is_err()
    {
        logln!(service, "[fs] cannot budget the fs structures");
        service.exit(3);
    }
    // SAFETY: mappings exclusive to this task; the shared frames have no
    // cached aliases on either side.
    unsafe {
        let mut mapped = PageTable(CPtr(TABLE_SLOT))
            .map(vspace, WINDOW_VA & !0x1F_FFFF)
            .is_ok();
        for index in 0..MAX_CLIENTS {
            mapped = mapped
                && Page(CPtr(SHARE_SLOT + index as u64))
                    .map(
                        vspace,
                        SHARE_VA + index * 0x1000,
                        RIGHTS_READ | RIGHTS_WRITE,
                        VM_CACHEABLE | VM_EXECUTE_NEVER,
                    )
                    .is_ok();
        }
        if !mapped {
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
    let mut device = BlockDevice {
        ep: block_ep,
        position: 0,
        disk_bytes: capacity * SECTOR_SIZE,
    };
    // Superblock probe: sector 0 carries either the FAT32 BPB ("FAT32" at
    // offset 82) or the ext4 superblock (magic 0x53EF at offset 0x438).
    // SAFETY: CLIENT_BUF_VA is exclusively mapped and only read here.
    // Four sectors: the ext4 superblock lives at bytes 1024..2048.
    if device.fetch(0, 4).is_err() {
        logln!(service, "[fs] cannot read the superblock");
        service.exit(6);
    }
    // SAFETY: as above; the window holds the four fetched sectors.
    let probe = unsafe { core::slice::from_raw_parts(CLIENT_BUF_VA as *const u8, 2048) };
    let ext_magic = u16::from_le_bytes([probe[0x438], probe[0x439]]);
    let kind = if &probe[82..90] == b"FAT32   " {
        "fat32"
    } else if ext_magic == 0xEF53 {
        "ext4"
    } else {
        logln!(service, "[fs] unknown filesystem superblock");
        service.exit(6);
    };
    let mut filesystem: Box<dyn FileSystem> = match kind {
        "fat32" => match FatVolume::open(device) {
            Ok(volume) => Box::new(backends::FatFs { volume }),
            Err(_) => {
                logln!(service, "[fs] mount failed");
                service.exit(6);
            }
        },
        _ => match Ext4Filesystem::new(device, FsConfig::default()) {
            Ok(fs) => Box::new(backends::Ext4Fs { fs }),
            Err(_) => {
                logln!(service, "[fs] mount failed");
                service.exit(6);
            }
        },
    };
    logln!(service, "[fs] superblock: {kind}");
    logln!(service, "[fs] mounted, {} sectors", capacity);

    // Acceptance hook (FAT_TEST=1): open hello, read its first sector and
    // log size plus checksum for the check script to compare with the image.
    if option_env!("FAT_TEST").is_some_and(|value| value == "1") {
        match filesystem.lookup(b"hello") {
            Some(mut file) => {
                // 4 KiB crosses cluster boundaries, so a looping FAT chain
                // surfaces as a read error instead of a first-cluster success.
                let mut buffer = [0u8; 4096];
                let read = filesystem.read_at(&mut file, 0, &mut buffer).unwrap_or(0);
                let sum: u32 = buffer[..read].iter().copied().map(u32::from).sum();
                logln!(service, "[fs] test size={}", file.size());
                logln!(service, "[fs] test read={read} sum={sum:#x}");
            }
            None => {
                logln!(service, "[fs] test file not found");
                service.exit(7);
            }
        }
    }

    // Per-client state: badge, granted shared page, private handle table.
    let mut clients: [Option<Client>; MAX_CLIENTS] = core::array::from_fn(|_| None);
    loop {
        let Ok(received) = ipc::recv(self_ep) else {
            continue;
        };
        match received.label {
            fs::BIND => {
                let version = received.word(0);
                // v2 serves long names and per-client buffers; v1 clients keep
                // their exact wire behaviour. Negotiate down to what the
                // client asked for so old clients accept the reply.
                if version == 0 || version > fs::PROTOCOL_VERSION {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                }
                let Some(slot) = clients.iter_mut().position(|slot| slot.is_none()) else {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                };
                // A badge names the client in the binding table. An unbadged
                // (badge 0) caller is admitted as the single anonymous client —
                // exactly the v1 single-client world, so unmodified v1
                // services keep binding — but a second one would be
                // indistinguishable from the first and is refused: concurrent
                // clients must mint their own badges (mysh: badge 1, its
                // children: badge 2).
                if received.badge == 0 && clients.iter().flatten().any(|client| client.badge == 0) {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                }
                let share_slot = SHARE_SLOT + slot as u64;
                let share_va = SHARE_VA + slot * 0x1000;
                let reply = ipc::reply_cap(status::OK, &[version, 0x1000], &[share_slot]);
                if reply.is_ok() {
                    clients[slot] = Some(Client {
                        badge: received.badge,
                        share_va,
                        files: alloc::boxed::Box::new(core::array::from_fn(|_| None)),
                    });
                    logln!(
                        service,
                        "[fs] client 0x{:x} bound as #{}",
                        received.badge,
                        slot
                    );
                }
            }
            fs::OPEN => {
                let Some(client) = client_by_badge_mut(&mut clients, received.badge) else {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                };
                let Some(name) = name_from_words(&received) else {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                };
                let Some(file) = filesystem.lookup(&name) else {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                };
                let Some(slot) = client.files.iter_mut().position(|slot| slot.is_none()) else {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                };
                let size = file.size();
                client.files[slot] = Some(file);
                let _ = ipc::reply(status::OK, &[slot as u64, size]);
            }
            fs::READ => {
                let Some(client) = client_by_badge_mut(&mut clients, received.badge) else {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                };
                let share_va = client.share_va;
                let file_id = received.word(0) as usize;
                let offset = received.word(1);
                let length = received.word(2) as usize;
                let Some(Some(file)) = client.files.get_mut(file_id) else {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                };
                if length == 0 || length > 0x1000 {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                }
                // SAFETY: the caller's shared page is exclusively mapped here
                // and the client reads it only after this reply arrives.
                let buffer =
                    unsafe { core::slice::from_raw_parts_mut(share_va as *mut u8, length) };
                match filesystem.read_at(file, offset, buffer) {
                    Ok(read) => {
                        let _ = ipc::reply(status::OK, &[read as u64]);
                    }
                    Err(_) => {
                        let _ = ipc::reply(status::ERROR, &[0]);
                    }
                }
            }
            fs::CLOSE => {
                let Some(client) = client_by_badge_mut(&mut clients, received.badge) else {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                };
                let file_id = received.word(0) as usize;
                if let Some(slot) = client.files.get_mut(file_id) {
                    *slot = None;
                }
                let _ = ipc::reply(status::OK, &[]);
            }
            fs::STAT => {
                let Some(name) = name_from_words(&received) else {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                };
                match filesystem.lookup(&name) {
                    Some(file) => {
                        let _ = ipc::reply(status::OK, &[file.size(), file.is_dir() as u64]);
                    }
                    None => {
                        let _ = ipc::reply(status::ERROR, &[0]);
                    }
                }
            }
            fs::READDIR => {
                let Some(client) = client_by_badge_mut(&mut clients, received.badge) else {
                    let _ = ipc::reply(status::ERROR, &[0]);
                    continue;
                };
                let start = received.word(0) as usize;
                let capacity = fs::DIR_ENTRIES_PER_PAGE;
                // SAFETY: the caller's shared page is exclusively mapped here
                // and the client reads it only after this reply arrives.
                let records = unsafe {
                    core::slice::from_raw_parts_mut(client.share_va as *mut fs::DirEntry, capacity)
                };
                let mut written = 0usize;
                let mut index = 0usize;
                let mut more = false;
                for (name, size, is_dir) in &filesystem.read_dir() {
                    let bytes = name.as_bytes();
                    if bytes.is_empty() || bytes.len() > 12 || bytes == b"." || bytes == b".." {
                        continue;
                    }
                    if index < start {
                        index += 1;
                        continue;
                    }
                    if written == capacity {
                        more = true;
                        break;
                    }
                    let mut record = fs::DirEntry {
                        name: [0; 12],
                        size: *size as u32,
                        is_dir: *is_dir as u32,
                    };
                    record.name[..bytes.len()].copy_from_slice(bytes);
                    records[written] = record;
                    written += 1;
                    index += 1;
                }
                let next = if more { (start + written) as u64 } else { 0 };
                let _ = ipc::reply(status::OK, &[written as u64, next]);
            }
            control::DEPENDENCY_LOST => {
                // The block service died: our cached volume and buffer die
                // with it. Exit and let init rebuild the chain.
                logln!(service, "[fs] dependency lost; exiting");
                service.exit(9);
            }
            control::STOP => {
                // Graceful stop. `control` and `fs` are disjoint label
                // segments, so this arm is reachable (previously shadowed by
                // `fs::STAT` when both were 0x104).
                let _ = ipc::reply(control::STOP_ACK, &[]);
                service.exit(0);
            }
            _ => {
                let _ = ipc::reply(status::ERROR, &[]);
            }
        }
    }
}


fn client_by_badge_mut(clients: &mut [Option<Client>], badge: u64) -> Option<&mut Client> {
    clients
        .iter_mut()
        .flatten()
        .find(|client| client.badge == badge)
}
