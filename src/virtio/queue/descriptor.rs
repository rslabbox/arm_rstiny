/// VirtIO 描述符标志
pub mod descriptor_flags {
    /// 描述符指向下一个描述符
    pub const NEXT: u16 = 1;
    /// 描述符是只写的（设备写入）
    pub const WRITE: u16 = 2;
    /// 描述符包含间接描述符表
    pub const INDIRECT: u16 = 4;
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Descriptor {
    /// 缓冲区地址（guest 物理地址）
    pub addr: u64,
    /// 缓冲区长度
    pub len: u32,
    /// 描述符标志
    pub flags: u16,
    /// 下一个描述符的索引（如果 NEXT 标志被设置）
    pub next: u16,
}
