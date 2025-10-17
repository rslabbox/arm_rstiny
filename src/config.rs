pub const PL011_UART_BASE: usize = 0xfeb50000;

pub const BOOT_KERNEL_STACK_SIZE: usize = 4096 * 4; // 16K
pub const PA_MAX_BITS: usize = 40; // 1TB
pub const VIRTIO_BASE_ADDR: usize = 0x0A00_0000;
pub const VIRTIO_SIZE: usize = 0x200; // 4K
pub const VIRTIO_COUNT: usize = 32;
pub const HEAP_ALLOCATOR_SIZE: usize = 0x1000000; // 16MB
