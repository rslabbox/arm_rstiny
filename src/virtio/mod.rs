// pub mod block;
pub mod constants;
pub mod error;
pub mod memory;
pub mod mmio;
pub mod queue;
pub mod utils;

pub use utils::{VirtioDeviceID, virtio_discover_device};

// use crate::virtio::queue::VirtQueue;

use crate::{
    utils::heap_allocator::global_allocator,
    virtio::{
        memory::VirtioAlloc,
        queue::{Queue, VirtQueue},
    },
};
use core::{alloc::Layout, ptr::NonNull};

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
    // let layout = Layout::new::<Queue>();
    // unsafe {
    //     let ptr = global_allocator()
    //         .lock()
    //         .allocate_first_fit(layout)
    //         .expect("Failed to allocate memory")
    //         .as_ptr() as *mut Queue;

    //     let instance = &*ptr;

    //     let (desc_addr, avail_addr, used_addr) = instance.get_addresses();
    //     info!(
    //         "VirtQueue addresses: desc=0x{:x}, avail=0x{:x}, used=0x{:x}",
    //         desc_addr, avail_addr, used_addr
    //     );
    // }

    let queue = VirtQueue::<DefaultVirtioAlloc>::new();
    let (desc_addr, avail_addr, used_addr) = queue.get_addresses();
    info!(
        "VirtQueue addresses: desc=0x{:x}, avail=0x{:x}, used=0x{:x}",
        desc_addr, avail_addr, used_addr
    );
}
