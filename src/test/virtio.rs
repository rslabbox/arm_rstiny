use core::{alloc::Layout, ptr::NonNull};

use crate::utils::heap_allocator::global_allocator;
use crate::virtio::block::VirtioBlkDevice;
use crate::virtio::queue::{VirtQueue, VirtioAlloc};
use crate::virtio::{VirtioDeviceID, virtio_discover_device};
use crate::virtio::constants::SECTOR_SIZE;
use super::fatfs::MyFileSystem;

use alloc::string::String;
use alloc::vec;
use aarch64_cpu::registers::{CNTFRQ_EL0, CNTVCT_EL0};
use aarch64_cpu::registers::Readable;
use fatfs::{FileSystem, FsOptions, Read, Seek, SeekFrom, Write};

// 实现一个具体的 VirtioAlloc
pub struct DefaultVirtioAlloc;

impl VirtioAlloc for DefaultVirtioAlloc {
    fn allocate(layout: Layout) -> NonNull<u8> {
        global_allocator()
            .lock()
            .allocate_first_fit(layout)
            .expect("Failed to allocate memory")
    }

    fn deallocate(ptr: NonNull<u8>, layout: Layout) {
        unsafe { global_allocator().lock().deallocate(ptr, layout) }
    }
}

#[inline(always)]
fn counter_hz() -> u64 { CNTFRQ_EL0.get() as u64 }
#[inline(always)]
fn counter_now() -> u64 { CNTVCT_EL0.get() as u64 }

pub fn virtio_test() {
    let queue = VirtQueue::<DefaultVirtioAlloc>::new(16);
    let (desc_addr, avail_addr, used_addr) = queue.get_addresses();
    info!(
        "VirtQueue addresses: desc=0x{:x}, avail=0x{:x}, used=0x{:x}",
        desc_addr, avail_addr, used_addr
    );

    let blk_addr = virtio_discover_device(VirtioDeviceID::Block).unwrap();
    let blk_dev = VirtioBlkDevice::<DefaultVirtioAlloc>::new(blk_addr)
        .expect("Failed to create VirtioBlkDevice");

    let myfs = MyFileSystem::new(blk_dev);
    let fs = FileSystem::new(myfs, FsOptions::new()).unwrap();
    let root_dir = fs.root_dir();
    let mut file = root_dir.create_file("hello.txt").expect("Failed to create file");
    file.write_all(b"Hello World!").expect("Failed to write to file");
    file.seek(SeekFrom::Start(0)).expect("Failed to seek in file");
    let mut buffer = vec![0u8; 12];
    file.read_exact(&mut buffer).expect("Failed to read from file");
    info!("Read from file: {:?}", String::from_utf8_lossy(&buffer));
}
