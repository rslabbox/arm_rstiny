/// Used Ring 元素
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UsedElement {
    /// 描述符链的头部索引
    pub id: u32,
    /// 写入的字节数
    pub len: u32,
}

/// Used Ring 结构
#[repr(C)]
#[repr(align(4096))] // 4096 = 0x1000
#[derive(Clone)]
pub struct UsedRing {
    /// 标志
    pub flags: u16,
    /// 索引
    pub idx: u16,
    /// 环形缓冲区
    pub ring: [UsedElement; 256],
    /// 用于事件抑制的索引（仅在 VIRTIO_F_EVENT_IDX 特性启用时使用）
    pub avail_event: u16,
}
