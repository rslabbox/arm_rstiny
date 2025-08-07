use core::{alloc::Layout, ptr::NonNull};

use crate::utils::heap_allocator::global_allocator;
use crate::virtio::block::VirtioBlkDevice;
use crate::virtio::queue::{VirtQueue, VirtioAlloc};
use crate::virtio::{VirtioDeviceID, virtio_discover_device};
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

pub fn virtio_test() {
    let queue = VirtQueue::<DefaultVirtioAlloc>::new();
    let (desc_addr, avail_addr, used_addr) = queue.get_addresses();
    info!(
        "VirtQueue addresses: desc=0x{:x}, avail=0x{:x}, used=0x{:x}",
        desc_addr, avail_addr, used_addr
    );

    let blk_addr = virtio_discover_device(VirtioDeviceID::Block).unwrap();
    let mut blk_dev = VirtioBlkDevice::<DefaultVirtioAlloc>::new(blk_addr)
        .expect("Failed to create VirtioBlkDevice");
    match blk_dev.read_sectors(0, 1) {
        Ok(data) => {
            info!("Read sector 0: {:?}", data);
        }
        Err(e) => {
            error!("Failed to read sector 0: {:?}", e);
        }
    }
}
