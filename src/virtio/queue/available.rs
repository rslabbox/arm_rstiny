use alloc::vec;
use alloc::vec::Vec;

/// Available Ring 结构
#[repr(C)]
#[derive(Debug, Clone)]
pub struct AvailableRing {
    /// 标志
    pub flags: u16,
    /// 索引
    pub idx: u16,
    /// 环形缓冲区
    pub ring: [u16; 256],
    /// 用于事件抑制的索引（仅在 VIRTIO_F_EVENT_IDX 特性启用时使用）
    pub used_event: u16,
}
