/// VirtIO Block feature flags
pub mod features {
    /// Device supports request barriers
    pub const VIRTIO_BLK_F_BARRIER: u64 = 1 << 0;
    /// Maximum size of any single segment is in size_max
    pub const VIRTIO_BLK_F_SIZE_MAX: u64 = 1 << 1;
    /// Maximum number of segments in a request is in seg_max
    pub const VIRTIO_BLK_F_SEG_MAX: u64 = 1 << 2;
    /// Disk-style geometry specified in geometry
    pub const VIRTIO_BLK_F_GEOMETRY: u64 = 1 << 4;
    /// Device is read-only
    pub const VIRTIO_BLK_F_RO: u64 = 1 << 5;
    /// Block size of disk is in blk_size
    pub const VIRTIO_BLK_F_BLK_SIZE: u64 = 1 << 6;
    /// Device supports scsi packet commands
    pub const VIRTIO_BLK_F_SCSI: u64 = 1 << 7;
    /// Cache flush command support
    pub const VIRTIO_BLK_F_FLUSH: u64 = 1 << 9;
    /// Device exports information on optimal I/O alignment
    pub const VIRTIO_BLK_F_TOPOLOGY: u64 = 1 << 10;
    /// Device can toggle its cache between writeback and writethrough modes
    pub const VIRTIO_BLK_F_CONFIG_WCE: u64 = 1 << 11;
    /// Device supports multiqueue
    pub const VIRTIO_BLK_F_MQ: u64 = 1 << 12;
    /// Device can support discard command
    pub const VIRTIO_BLK_F_DISCARD: u64 = 1 << 13;
    /// Device can support write zeroes command
    pub const VIRTIO_BLK_F_WRITE_ZEROES: u64 = 1 << 14;
}

/// VirtIO Block request types
pub mod request_type {
    /// Read request
    pub const VIRTIO_BLK_T_IN: u32 = 0;
    /// Write request
    pub const VIRTIO_BLK_T_OUT: u32 = 1;
    /// Flush request
    pub const VIRTIO_BLK_T_FLUSH: u32 = 4;
    /// Discard request
    pub const VIRTIO_BLK_T_DISCARD: u32 = 11;
    /// Write zeroes request
    pub const VIRTIO_BLK_T_WRITE_ZEROES: u32 = 13;
}

/// VirtIO Block status codes
pub mod status {
    /// Success
    pub const VIRTIO_BLK_S_OK: u8 = 0;
    /// I/O error
    pub const VIRTIO_BLK_S_IOERR: u8 = 1;
    /// Unsupported request
    pub const VIRTIO_BLK_S_UNSUPP: u8 = 2;
}

/// Standard sector size for block devices
pub const SECTOR_SIZE: usize = 512;

/// VirtIO MMIO register offsets
pub mod mmio {
    /// Magic value register
    pub const MAGIC_VALUE: usize = 0x000;
    /// Version register
    pub const VERSION: usize = 0x004;
    /// Device ID register
    pub const DEVICE_ID: usize = 0x008;
    /// Vendor ID register
    pub const VENDOR_ID: usize = 0x00c;
    /// Device features register
    pub const DEVICE_FEATURES: usize = 0x010;
    /// Device features selector
    pub const DEVICE_FEATURES_SEL: usize = 0x014;
    /// Driver features register
    pub const DRIVER_FEATURES: usize = 0x020;
    /// Driver features selector
    pub const DRIVER_FEATURES_SEL: usize = 0x024;
    /// Queue size register
    pub const QUEUE_SIZE: usize = 0x028;
    /// Queue selector
    pub const QUEUE_SEL: usize = 0x030;
    /// Queue size max
    pub const QUEUE_NUM_MAX: usize = 0x034;
    /// Queue size
    pub const QUEUE_NUM: usize = 0x038;
    /// Queue Align
    pub const QUEUE_ALIGN: usize = 0x3c;
    /// Queue PFN
    pub const QUEUE_DEVICE_PFN: usize = 0x040;
    /// Queue ready
    pub const QUEUE_READY: usize = 0x044;
    /// Queue notify
    pub const QUEUE_NOTIFY: usize = 0x050;
    /// Interrupt status
    pub const INTERRUPT_STATUS: usize = 0x060;
    /// Interrupt acknowledge
    pub const INTERRUPT_ACK: usize = 0x064;
    /// Device status
    pub const STATUS: usize = 0x070;
    /// Queue descriptor low
    pub const QUEUE_DESC_LOW: usize = 0x080;
    /// Queue descriptor high
    pub const QUEUE_DESC_HIGH: usize = 0x084;
    /// Queue driver low
    pub const QUEUE_DRIVER_LOW: usize = 0x090;
    /// Queue driver high
    pub const QUEUE_DRIVER_HIGH: usize = 0x094;
    /// Queue device low
    pub const QUEUE_DEVICE_LOW: usize = 0x0a0;
    /// Queue device high
    pub const QUEUE_DEVICE_HIGH: usize = 0x0a4;
    /// Configuration generation
    pub const CONFIG_GENERATION: usize = 0x0fc;
    /// Device-specific configuration
    pub const CONFIG: usize = 0x100;
}

/// VirtIO device status bits
pub mod device_status {
    /// Indicates that the guest OS has found the device and recognized it as a valid virtio device
    pub const ACKNOWLEDGE: u32 = 1;
    /// Indicates that the guest OS knows how to drive the device
    pub const DRIVER: u32 = 2;
    /// Indicates that something went wrong in the guest, and it has given up on the device
    pub const FAILED: u32 = 128;
    /// Indicates that the driver has acknowledged all the features it understands
    pub const FEATURES_OK: u32 = 8;
    /// Indicates that the driver is set up and ready to drive the device
    pub const DRIVER_OK: u32 = 4;
    /// Indicates that the device needs a reset
    pub const DEVICE_NEEDS_RESET: u32 = 64;
}

/// VirtIO Block device ID
pub const VIRTIO_DEVICE_ID_BLOCK: u32 = 2;

/// VirtIO magic value
pub const VIRTIO_MMIO_MAGIC: u32 = 0x74726976; // "virt"

/// VirtIO version (we support both version 1 and 2)
pub const VIRTIO_MMIO_VERSION_1: u32 = 1;
pub const VIRTIO_MMIO_VERSION_2: u32 = 2;
