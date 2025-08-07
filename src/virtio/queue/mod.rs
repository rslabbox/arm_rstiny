mod available;
pub mod descriptor;
mod queue;
mod used;

use core::{alloc::Layout, marker::PhantomData, ptr::NonNull};

pub use descriptor::Descriptor;

pub use super::{memory::VirtioAlloc, queue::queue::Queue};

pub struct VirtQueue<M: VirtioAlloc> {
    queue: NonNull<Queue>,
    pub size: u16,
    _marker: PhantomData<M>,
}

impl<M: VirtioAlloc> VirtQueue<M> {
    pub fn new() -> Self {
        let layout = Layout::new::<Queue>();
        let queue_ptr = M::allocate(layout).as_ptr() as *mut Queue;
        Self {
            queue: NonNull::new(queue_ptr).expect("Failed to create VirtQueue"),
            size: 16,
            _marker: PhantomData,
        }
    }

    // 获取 queue 的引用
    pub fn queue(&self) -> &Queue {
        unsafe { self.queue.as_ref() }
    }

    // 获取 queue 的可变引用
    pub fn queue_mut(&mut self) -> &mut Queue {
        unsafe { self.queue.as_mut() }
    }

    /// 获取队列的物理地址信息（用于设备配置）
    pub fn get_addresses(&self) -> (u64, u64, u64) {
        let desc_addr = self.queue().descriptors.as_ptr() as u64;
        let avail_addr = &self.queue().available as *const _ as u64;
        let used_addr = &self.queue().used as *const _ as u64;

        (desc_addr, avail_addr, used_addr)
    }
}

impl<M: VirtioAlloc> Drop for VirtQueue<M> {
    fn drop(&mut self) {
        let layout = Layout::new::<Queue>();
        let _ = self.queue_mut();
        let non_null_ptr = NonNull::new(self.queue.as_ptr() as *mut u8)
            .expect("Failed to create NonNull pointer for deallocation");
        M::deallocate(non_null_ptr, layout);
    }
}
