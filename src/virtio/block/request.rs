//! VirtIO Block Device Request Structures
//!
//! This module defines the request and response structures used for
//! communicating with VirtIO Block devices.

use alloc::vec;
use alloc::vec::Vec;

use crate::virtio::constants::{SECTOR_SIZE, request_type, status};

/// VirtIO Block request header
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtioBlkReqHeader {
    /// Request type (VIRTIO_BLK_T_*)
    pub req_type: u32,
    /// Reserved field
    pub reserved: u32,
    /// Starting sector for the operation
    pub sector: u64,
}

impl VirtioBlkReqHeader {
    /// Create a new read request header
    pub fn new_read(sector: u64) -> Self {
        Self {
            req_type: request_type::VIRTIO_BLK_T_IN,
            reserved: 0,
            sector,
        }
    }

    /// Create a new write request header
    pub fn new_write(sector: u64) -> Self {
        Self {
            req_type: request_type::VIRTIO_BLK_T_OUT,
            reserved: 0,
            sector,
        }
    }

    /// Create a new flush request header
    pub fn new_flush() -> Self {
        Self {
            req_type: request_type::VIRTIO_BLK_T_FLUSH,
            reserved: 0,
            sector: 0,
        }
    }
}

/// VirtIO Block request status
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtioBlkReqStatus {
    /// Status code (VIRTIO_BLK_S_*)
    pub status: u8,
}

impl VirtioBlkReqStatus {
    /// Create a new status structure
    pub fn new() -> Self {
        Self {
            status: status::VIRTIO_BLK_S_OK,
        }
    }

    /// Check if the request was successful
    pub fn is_ok(&self) -> bool {
        self.status == status::VIRTIO_BLK_S_OK
    }

    /// Check if there was an I/O error
    pub fn is_io_error(&self) -> bool {
        self.status == status::VIRTIO_BLK_S_IOERR
    }

    /// Check if the request was unsupported
    pub fn is_unsupported(&self) -> bool {
        self.status == status::VIRTIO_BLK_S_UNSUPP
    }

    /// Get a human-readable description of the status
    pub fn description(&self) -> &'static str {
        match self.status {
            status::VIRTIO_BLK_S_OK => "Success",
            status::VIRTIO_BLK_S_IOERR => "I/O Error",
            status::VIRTIO_BLK_S_UNSUPP => "Unsupported",
            _ => "Unknown",
        }
    }
}

impl Default for VirtioBlkReqStatus {
    fn default() -> Self {
        Self::new()
    }
}

/// Complete VirtIO Block request structure
#[derive(Debug)]
pub struct VirtioBlkRequest {
    /// Request header
    pub header: VirtioBlkReqHeader,
    /// Data buffer (for read/write operations)
    pub data: Vec<u8>,
    /// Request status
    pub status: VirtioBlkReqStatus,
}

impl VirtioBlkRequest {
    /// Create a new read request
    pub fn new_read(sector: u64, num_sectors: usize) -> Self {
        let data_size = num_sectors * SECTOR_SIZE;
        Self {
            header: VirtioBlkReqHeader::new_read(sector),
            data: vec![0u8; data_size],
            status: VirtioBlkReqStatus::new(),
        }
    }

    /// Create a new write request
    pub fn new_write(sector: u64, data: Vec<u8>) -> Self {
        Self {
            header: VirtioBlkReqHeader::new_write(sector),
            data,
            status: VirtioBlkReqStatus::new(),
        }
    }

    /// Create a new flush request
    pub fn new_flush() -> Self {
        Self {
            header: VirtioBlkReqHeader::new_flush(),
            data: Vec::new(),
            status: VirtioBlkReqStatus::new(),
        }
    }

    /// Get the number of sectors this request covers
    pub fn num_sectors(&self) -> usize {
        if self.data.is_empty() {
            0
        } else {
            (self.data.len() + SECTOR_SIZE - 1) / SECTOR_SIZE
        }
    }

    /// Check if this is a read request
    pub fn is_read(&self) -> bool {
        self.header.req_type == request_type::VIRTIO_BLK_T_IN
    }

    /// Check if this is a write request
    pub fn is_write(&self) -> bool {
        self.header.req_type == request_type::VIRTIO_BLK_T_OUT
    }

    /// Check if this is a flush request
    pub fn is_flush(&self) -> bool {
        self.header.req_type == request_type::VIRTIO_BLK_T_FLUSH
    }

    /// Get the starting sector
    pub fn sector(&self) -> u64 {
        self.header.sector
    }

    /// Get the data buffer
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Get mutable data buffer
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// Get the request status
    pub fn status(&self) -> &VirtioBlkReqStatus {
        &self.status
    }

    /// Check if the request completed successfully
    pub fn is_successful(&self) -> bool {
        self.status.is_ok()
    }
}

/// Descriptor chain information for a VirtIO Block request
#[derive(Debug)]
pub struct VirtioBlkDescriptorChain {
    /// Descriptor indices used for this request
    pub descriptors: Vec<u16>,
    /// Total length of the request
    pub total_len: u32,
}

impl VirtioBlkDescriptorChain {
    /// Create a new descriptor chain
    pub fn new() -> Self {
        Self {
            descriptors: Vec::new(),
            total_len: 0,
        }
    }

    /// Add a descriptor to the chain
    pub fn add_descriptor(&mut self, desc_idx: u16, len: u32) {
        self.descriptors.push(desc_idx);
        self.total_len += len;
    }

    /// Get the head descriptor index
    pub fn head(&self) -> Option<u16> {
        self.descriptors.first().copied()
    }

    /// Get the number of descriptors in the chain
    pub fn len(&self) -> usize {
        self.descriptors.len()
    }

    /// Check if the chain is empty
    pub fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }
}

impl Default for VirtioBlkDescriptorChain {
    fn default() -> Self {
        Self::new()
    }
}
