//! VirtIO Block Device Configuration
//!
//! This module defines the configuration structures, constants, and register
//! offsets for VirtIO Block devices according to the VirtIO specification.

/// VirtIO Block device configuration space
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtioBlkConfig {
    /// The capacity of the device (expressed in 512-byte sectors)
    pub capacity: u64,
    /// The maximum segment size (if VIRTIO_BLK_F_SIZE_MAX is negotiated)
    pub size_max: u32,
    /// The maximum number of segments (if VIRTIO_BLK_F_SEG_MAX is negotiated)
    pub seg_max: u32,
    /// Geometry of the device (if VIRTIO_BLK_F_GEOMETRY is negotiated)
    pub geometry: VirtioBlkGeometry,
    /// Block size of the device (if VIRTIO_BLK_F_BLK_SIZE is negotiated)
    pub blk_size: u32,
    /// Topology of the device (if VIRTIO_BLK_F_TOPOLOGY is negotiated)
    pub topology: VirtioBlkTopology,
    /// Writeback mode (if VIRTIO_BLK_F_CONFIG_WCE is negotiated)
    pub writeback: u8,
    /// Number of vqs (if VIRTIO_BLK_F_MQ is negotiated)
    pub num_queues: u16,
    /// Maximum discard sectors (if VIRTIO_BLK_F_DISCARD is negotiated)
    pub max_discard_sectors: u32,
    /// Maximum discard segments (if VIRTIO_BLK_F_DISCARD is negotiated)
    pub max_discard_seg: u32,
    /// Discard sector alignment (if VIRTIO_BLK_F_DISCARD is negotiated)
    pub discard_sector_alignment: u32,
    /// Maximum write zeroes sectors (if VIRTIO_BLK_F_WRITE_ZEROES is negotiated)
    pub max_write_zeroes_sectors: u32,
    /// Maximum write zeroes segments (if VIRTIO_BLK_F_WRITE_ZEROES is negotiated)
    pub max_write_zeroes_seg: u32,
    /// Write zeroes may unmap (if VIRTIO_BLK_F_WRITE_ZEROES is negotiated)
    pub write_zeroes_may_unmap: u8,
}

/// VirtIO Block device geometry
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtioBlkGeometry {
    /// Number of cylinders
    pub cylinders: u16,
    /// Number of heads
    pub heads: u8,
    /// Number of sectors per track
    pub sectors: u8,
}

/// VirtIO Block device topology
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtioBlkTopology {
    /// Physical block exponent
    pub physical_block_exp: u8,
    /// Alignment offset
    pub alignment_offset: u8,
    /// Minimum I/O size
    pub min_io_size: u16,
    /// Optimal I/O size
    pub opt_io_size: u32,
}
