use alloc::vec::Vec;

use super::available::AvailableRing;
use super::descriptor::Descriptor;
use super::used::UsedRing;

/// VirtIO 队列
#[repr(C)]
#[repr(align(4096))] // 4096 = 0x1000
#[derive(Clone)]
pub struct Queue {
    /// 描述符表
    pub descriptors: [Descriptor; 16],
    /// 可用环
    pub available: AvailableRing,
    /// 已使用环
    pub used: UsedRing,

    pub size: u16,

    /// 空闲描述符列表
    pub free_descriptors: Vec<u16>,
    /// 最后处理的已使用索引
    pub last_used_idx: u16,
}

impl Queue {
    /// 获取队列的物理地址信息（用于设备配置）
    pub fn get_addresses(&self) -> (u64, u64, u64) {
        let desc_addr = self.descriptors.as_ptr() as u64;
        let avail_addr = &self.available as *const _ as u64;
        let used_addr = &self.used as *const _ as u64;

        (desc_addr, avail_addr, used_addr)
    }
}
