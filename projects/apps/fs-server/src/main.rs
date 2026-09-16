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

use embedded_io::{ErrorKind, Read as IoRead, Seek as IoSeek, SeekFrom};
use hadris_fat::sync::{FatVolume, FatVolumeReadExt, FileEntry};
use lwext4_rust::{DummyHal, Ext4Error, Ext4Filesystem, Ext4Result, FileAttr, FsConfig, InodeType};
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
// shared buffer and one buffer page per fs client.
const WINDOW_VA: usize = 0x0400_0000;
const CLIENT_BUF_VA: usize = WINDOW_VA; // block's shared buffer (received)
const SHARE_VA: usize = WINDOW_VA + 0x1000; // first per-client page
const TABLE_SLOT: u64 = 44;
const RECV_SLOT: u64 = 60; // landing slot for block's BIND cap transfer
const SHARE_SLOT: u64 = 61; // first frame granted to an fs client on BIND
const MAX_FILES: usize = 4;
const MAX_CLIENTS: usize = fs::MAX_CLIENTS as usize;
const SECTOR_SIZE: u64 = 512;
const SECTORS_PER_READ: u64 = 8;

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

/// The filesystem backends share one fs v2 protocol; the trait is the seam
/// between the wire protocol and the on-disk format. FAT32 rides hadris-fat;
/// ext4 is read-only through lwext4 (journal-less, cleanly-unmounted images).
enum OpenFile {
    Fat(FileEntry),
    Ext4 { ino: u32, size: u64 },
}
impl OpenFile {
    fn size(&self) -> u64 {
        match self {
            OpenFile::Fat(entry) => entry.len(),
            OpenFile::Ext4 { size, .. } => *size,
        }
    }
    fn is_dir(&self) -> bool {
        match self {
            OpenFile::Fat(entry) => entry.is_directory(),
            OpenFile::Ext4 { .. } => false,
        }
    }
}

trait FileSystem {
    /// Resolve a root-directory file name; directories are not openable.
    fn lookup(&mut self, name: &[u8]) -> Option<OpenFile>;
    /// Read up to `buf.len()` bytes at `offset`.
    fn read_at(
        &mut self,
        file: &mut OpenFile,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, ErrorKind>;
    /// Every root-directory entry as (name, size, is-dir); the caller applies
    /// the wire-format filters (name length, pagination).
    fn read_dir(&mut self) -> Vec<(alloc::string::String, u64, bool)>;
}

struct FatFs {
    volume: FatVolume<BlockDevice>,
}
impl FileSystem for FatFs {
    fn lookup(&mut self, name: &[u8]) -> Option<OpenFile> {
        let dir = self.volume.root_dir();
        for entry in dir.entries() {
            let Ok(entry) = entry else { continue };
            // FAT names are case-insensitive: `hello` matches a short name
            // stored as `HELLO` (and vice versa). `entry.name()` yields the
            // long name when the directory carries an LFN record (P2.1).
            if entry.name().as_bytes().eq_ignore_ascii_case(name)
                && entry.as_entry().is_some_and(|file| file.is_file())
            {
                return Some(OpenFile::Fat(entry.as_entry()?.clone()));
            }
        }
        None
    }
    fn read_at(
        &mut self,
        file: &mut OpenFile,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, ErrorKind> {
        let OpenFile::Fat(entry) = file else {
            return Err(ErrorKind::InvalidInput);
        };
        let mut reader = self.volume.read_file(entry).map_err(|_| ErrorKind::Other)?;
        reader.seek(SeekFrom::Start(offset)).map_err(|_| ErrorKind::Other)?;
        reader.read(buf).map_err(|_| ErrorKind::Other)
    }
    fn read_dir(&mut self) -> Vec<(alloc::string::String, u64, bool)> {
        let dir = self.volume.root_dir();
        let mut entries = Vec::new();
        for entry in dir.entries() {
            let Ok(entry) = entry else { continue };
            let Some(file) = entry.as_entry() else { continue };
            entries.push((
                alloc::string::String::from_utf8_lossy(entry.name().as_bytes()).into_owned(),
                file.len(),
                file.is_directory(),
            ));
        }
        entries
    }
}

/// ext4 through lwext4, read-only: the image must be journal-less (built with
/// `mke2fs -O ^has_journal`) and cleanly unmounted, so mounting and serving
/// never write. Root is inode 2; all block reads travel the same block IPC
/// the FAT backend uses.
struct Ext4Fs {
    fs: Ext4Filesystem<DummyHal, BlockDevice>,
}
impl FileSystem for Ext4Fs {
    fn lookup(&mut self, name: &[u8]) -> Option<OpenFile> {
        // ext4 names are case-sensitive UTF-8 (unlike the FAT backend's
        // case-insensitive match).
        let Ok(text) = core::str::from_utf8(name) else {
            return None;
        };
        let mut found = self.fs.lookup(2, text).ok()?;
        let ino = found.entry().ino();
        let mut attr = FileAttr::default();
        self.fs.get_attr(ino, &mut attr).ok()?;
        if attr.node_type == InodeType::Directory {
            return None; // fs v2 opens regular files only
        }
        Some(OpenFile::Ext4 {
            ino,
            size: attr.size,
        })
    }
    fn read_at(
        &mut self,
        file: &mut OpenFile,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, ErrorKind> {
        let OpenFile::Ext4 { ino, .. } = file else {
            return Err(ErrorKind::InvalidInput);
        };
        self.fs.read_at(*ino, buf, offset).map_err(|_| ErrorKind::Other)
    }
    fn read_dir(&mut self) -> Vec<(alloc::string::String, u64, bool)> {
        let mut entries = Vec::new();
        let Ok(mut reader) = self.fs.read_dir(2, 0) else {
            return entries;
        };
        while let Some(entry) = reader.current() {
            let name = alloc::string::String::from_utf8_lossy(entry.name()).into_owned();
            let ino = entry.ino();
            let is_dir = entry.inode_type() == InodeType::Directory;
            let mut attr = FileAttr::default();
            let _ = self.fs.get_attr(ino, &mut attr);
            entries.push((name, attr.size, is_dir));
            if reader.step().is_err() {
                break;
            }
        }
        entries
    }
}

/// The same device behind the lwext4 block interface: reads copy out of the
/// block service's shared buffer; writes are refused (the acceptance images
/// are read-only and the QEMU drive is `readonly=on`).
impl lwext4_rust::BlockDevice for BlockDevice {
    fn read_blocks(&mut self, block_id: u64, buf: &mut [u8]) -> Ext4Result<usize> {
        let sectors = (buf.len().div_ceil(SECTOR_SIZE as usize)) as u64;
        let mut copied = 0usize;
        while copied < buf.len() {
            let lba = block_id + copied as u64 / SECTOR_SIZE;
            let count = (sectors - copied as u64 / SECTOR_SIZE as usize as u64).min(SECTORS_PER_READ)
                .min((self.disk_bytes - lba * SECTOR_SIZE) / SECTOR_SIZE);
            if count == 0 {
                break;
            }
            if self.fetch(lba, count).is_err() {
                return Err(Ext4Error::new(-5, None));
            }
            // SAFETY: CLIENT_BUF_VA is exclusively mapped and the block
            // service only writes it while this task is blocked in the call.
            let source = unsafe {
                core::slice::from_raw_parts(
                    (CLIENT_BUF_VA + (lba * SECTOR_SIZE - lba * SECTOR_SIZE) as usize) as *const u8,
                    (count * SECTOR_SIZE) as usize,
                )
            };
            let take = (count * SECTOR_SIZE) as usize;
            buf[copied..copied + take].copy_from_slice(&source[..take]);
            copied += take;
        }
        Ok(copied)
    }

    // lwext4's mount writes superblock state (mount count, fs state). The
    // disk is read-only, so the write is accepted and discarded: the read-only
    // consumer never depends on it, and the acceptance images are built
    // cleanly (no journal to replay). Reads never depend on these writes.
    fn write_blocks(&mut self, _block_id: u64, buf: &[u8]) -> Ext4Result<usize> {
        Ok(buf.len())
    }

    fn num_blocks(&self) -> Ext4Result<u64> {
        Ok(self.disk_bytes / SECTOR_SIZE)
    }
}

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
    logln!(service, "[fs] probe ext_magic={:#06x} b82={:02x?}", ext_magic, &probe[82..90]);
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
            Ok(volume) => Box::new(FatFs { volume }),
            Err(_) => {
                logln!(service, "[fs] mount failed");
                service.exit(6);
            }
        },
        _ => {
            logln!(service, "[fs] ext4: constructing");
            match Ext4Filesystem::new(device, FsConfig::default()) {
                Ok(fs) => {
                    logln!(service, "[fs] ext4: constructed");
                    Box::new(Ext4Fs { fs })
                }
                Err(error) => {
                    logln!(service, "[fs] ext4 mount failed: code={}", error.code);
                    service.exit(6);
                }
            }
        }
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
                let Some(mut file) = filesystem.lookup(&name) else {
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
