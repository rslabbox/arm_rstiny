//! VirtIO Block Device Driver
//!
//! This module implements a VirtIO Block device driver for reading and writing
//! block data according to the VirtIO specification.

pub mod config;
pub mod request;

use super::queue::VirtioAlloc;
use crate::virtio::constants::{
    SECTOR_SIZE, VIRTIO_DEVICE_ID_BLOCK, VIRTIO_MMIO_MAGIC, VIRTIO_MMIO_VERSION_1,
    VIRTIO_MMIO_VERSION_2, device_status, features, mmio,
};
use crate::virtio::error::{VirtioError, VirtioResult};
use crate::virtio::mmio::{read_mmio_u32, write_mmio_u32};
use crate::virtio::queue::{Descriptor, VirtQueue, descriptor::descriptor_flags};
use alloc::vec::Vec;
use config::*;
use log::{debug, error, info};
use request::*;

/// VirtIO Block device driver
pub struct VirtioBlkDevice<M: VirtioAlloc> {
    /// Base address of the device's MMIO registers
    base_addr: usize,
    /// VirtIO queue for block operations
    virtqueue: VirtQueue<M>,
    /// Device configuration
    config: VirtioBlkConfig,
    /// Device features
    features: u64,
    /// Block version
    version: u32,
}

impl<M: VirtioAlloc> VirtioBlkDevice<M> {
    /// Create a new VirtIO Block device
    pub fn new(base_addr: usize) -> VirtioResult<Self> {
        let mut device = Self {
            base_addr,
            virtqueue: VirtQueue::new(), // Start with a small queue
            config: VirtioBlkConfig::new(),
            features: 0,
            version: VIRTIO_MMIO_VERSION_1, // Default to version 1
        };

        device.init()?;
        Ok(device)
    }

    /// Initialize the VirtIO Block device
    fn init(&mut self) -> VirtioResult<()> {
        info!("Initializing VirtIO Block device at 0x{:x}", self.base_addr);

        // Check magic value
        let magic = read_mmio_u32(self.base_addr, mmio::MAGIC_VALUE);
        if magic != VIRTIO_MMIO_MAGIC {
            error!("Invalid VirtIO magic value: 0x{:x}", magic);
            return Err(VirtioError::InvalidConfig);
        }

        // Check version (support both version 1 and 2)
        let version = read_mmio_u32(self.base_addr, mmio::VERSION);
        if version != VIRTIO_MMIO_VERSION_1 && version != VIRTIO_MMIO_VERSION_2 {
            error!("Unsupported VirtIO version: {}", version);
            return Err(VirtioError::NotSupported);
        }
        self.version = version;
        debug!("VirtIO version: {}", self.version);

        // Check device ID
        let device_id = read_mmio_u32(self.base_addr, mmio::DEVICE_ID);
        if device_id != VIRTIO_DEVICE_ID_BLOCK {
            error!("Not a VirtIO Block device: {}", device_id);
            return Err(VirtioError::InvalidDeviceIndex);
        }

        info!("Found VirtIO Block device (ID: {})", device_id);

        // Reset device
        write_mmio_u32(self.base_addr, mmio::STATUS, 0);

        // Set ACKNOWLEDGE status
        self.set_status(device_status::ACKNOWLEDGE);

        // Set DRIVER status
        self.set_status(device_status::ACKNOWLEDGE | device_status::DRIVER);

        // Read and negotiate features
        self.negotiate_features()?;

        // Set FEATURES_OK status
        self.set_status(
            device_status::ACKNOWLEDGE | device_status::DRIVER | device_status::FEATURES_OK,
        );

        // Verify FEATURES_OK
        let status = read_mmio_u32(self.base_addr, mmio::STATUS);
        if status & device_status::FEATURES_OK == 0 {
            error!("Device rejected our feature set");
            return Err(VirtioError::FeatureNegotiationFailed);
        }

        // Read device configuration
        self.read_config()?;

        // Setup queue
        self.setup_queue()?;

        // Set DRIVER_OK status
        self.set_status(
            device_status::ACKNOWLEDGE
                | device_status::DRIVER
                | device_status::FEATURES_OK
                | device_status::DRIVER_OK,
        );

        info!(
            "VirtIO Block device initialized successfully. Capacity: {} sectors ({} MB)",
            self.config.capacity,
            (self.config.capacity * SECTOR_SIZE as u64) / (1024 * 1024)
        );

        Ok(())
    }

    /// Negotiate device features
    fn negotiate_features(&mut self) -> VirtioResult<()> {
        // Read device features
        write_mmio_u32(self.base_addr, mmio::DEVICE_FEATURES_SEL, 0);
        let device_features_low = read_mmio_u32(self.base_addr, mmio::DEVICE_FEATURES);
        write_mmio_u32(self.base_addr, mmio::DEVICE_FEATURES_SEL, 1);
        let device_features_high = read_mmio_u32(self.base_addr, mmio::DEVICE_FEATURES);

        let device_features = (device_features_high as u64) << 32 | device_features_low as u64;
        debug!("Device features: 0x{:x}", device_features);

        // Select features we want to use
        let driver_features = 0u64;

        // We want basic block functionality
        // if device_features & features::VIRTIO_BLK_F_SIZE_MAX != 0 {
        //     driver_features |= features::VIRTIO_BLK_F_SIZE_MAX;
        // }
        // if device_features & features::VIRTIO_BLK_F_SEG_MAX != 0 {
        //     driver_features |= features::VIRTIO_BLK_F_SEG_MAX;
        // }
        // if device_features & features::VIRTIO_BLK_F_BLK_SIZE != 0 {
        //     driver_features |= features::VIRTIO_BLK_F_BLK_SIZE;
        // }

        self.features = driver_features;
        debug!("Driver features: 0x{:x}", driver_features);

        // Write driver features
        write_mmio_u32(self.base_addr, mmio::DRIVER_FEATURES_SEL, 0);
        write_mmio_u32(
            self.base_addr,
            mmio::DRIVER_FEATURES,
            driver_features as u32,
        );
        write_mmio_u32(self.base_addr, mmio::DRIVER_FEATURES_SEL, 1);
        write_mmio_u32(
            self.base_addr,
            mmio::DRIVER_FEATURES,
            (driver_features >> 32) as u32,
        );

        Ok(())
    }

    /// Read device configuration
    fn read_config(&mut self) -> VirtioResult<()> {
        // Read capacity (first 8 bytes of config space)
        let capacity_low = read_mmio_u32(self.base_addr, mmio::CONFIG);
        let capacity_high = read_mmio_u32(self.base_addr, mmio::CONFIG + 4);
        self.config.capacity = (capacity_high as u64) << 32 | capacity_low as u64;

        // Read block size if supported
        if self.features & features::VIRTIO_BLK_F_BLK_SIZE != 0 {
            self.config.blk_size = read_mmio_u32(self.base_addr, mmio::CONFIG + 20); // blk_size offset
        }

        debug!("Device capacity: {} sectors", self.config.capacity);
        debug!("Block size: {} bytes", self.config.blk_size);

        Ok(())
    }

    /// Setup the VirtIO queue
    fn setup_queue(&mut self) -> VirtioResult<()> {
        // Select queue 0 (the only queue for block devices)
        write_mmio_u32(self.base_addr, mmio::QUEUE_SEL, 0);

        // Check maximum queue size
        let max_queue_size = read_mmio_u32(self.base_addr, mmio::QUEUE_NUM_MAX);
        if max_queue_size == 0 {
            error!("Queue 0 is not available");
            return Err(VirtioError::InvalidQueue);
        }

        debug!("Maximum queue size: {}", max_queue_size);

        // Set queue size (use minimum of our size and max size)
        let queue_size = core::cmp::min(self.virtqueue.size as u32, max_queue_size);
        write_mmio_u32(self.base_addr, mmio::QUEUE_NUM, queue_size);
        self.virtqueue.size = queue_size as u16;

        // Initialize free descriptor list
        self.virtqueue.queue_mut().free_descriptors.clear();
        for i in 0..self.virtqueue.size {
            self.virtqueue.queue_mut().free_descriptors.push(i);
        }

        // Get queue addresses
        let (desc_addr, avail_addr, used_addr) = self.virtqueue.get_addresses();

        info!(
            "desc_addr: 0x{:x}, avail_addr: 0x{:x}, used_addr: 0x{:x}",
            desc_addr, avail_addr, used_addr
        );

        if self.version == VIRTIO_MMIO_VERSION_2 {
            // Set queue addresses
            write_mmio_u32(self.base_addr, mmio::QUEUE_DESC_LOW, desc_addr as u32);
            write_mmio_u32(
                self.base_addr,
                mmio::QUEUE_DESC_HIGH,
                (desc_addr >> 32) as u32,
            );
            write_mmio_u32(self.base_addr, mmio::QUEUE_DRIVER_LOW, avail_addr as u32);
            write_mmio_u32(
                self.base_addr,
                mmio::QUEUE_DRIVER_HIGH,
                (avail_addr >> 32) as u32,
            );
            write_mmio_u32(self.base_addr, mmio::QUEUE_DEVICE_LOW, used_addr as u32);
            write_mmio_u32(
                self.base_addr,
                mmio::QUEUE_DEVICE_HIGH,
                (used_addr >> 32) as u32,
            );
        } else {
            // Set queue addresses for Legacy mode
            write_mmio_u32(self.base_addr, mmio::QUEUE_SIZE, 4096);
            write_mmio_u32(
                self.base_addr,
                mmio::QUEUE_DESC_HIGH,
                (desc_addr >> 32) as u32,
            );
            write_mmio_u32(self.base_addr, mmio::QUEUE_ALIGN, 4096);
            write_mmio_u32(
                self.base_addr,
                mmio::QUEUE_DEVICE_PFN,
                (desc_addr >> 12) as u32,
            );
        }

        // Enable the queue
        write_mmio_u32(self.base_addr, mmio::QUEUE_READY, 1);

        debug!("Queue setup complete");
        Ok(())
    }

    /// Set device status
    fn set_status(&self, status: u32) {
        write_mmio_u32(self.base_addr, mmio::STATUS, status);
    }

    /// Read sectors from the block device
    pub fn read_sectors(&mut self, sector: u64, num_sectors: usize) -> VirtioResult<Vec<u8>> {
        if sector + num_sectors as u64 > self.config.capacity {
            error!(
                "Read beyond device capacity: sector {} + {} > {}",
                sector, num_sectors, self.config.capacity
            );
            return Err(VirtioError::InvalidSector);
        }

        debug!(
            "Reading {} sectors starting from sector {}",
            num_sectors, sector
        );

        // Create a read request
        let mut request = VirtioBlkRequest::new_read(sector, num_sectors);

        // Submit the request and wait for completion
        self.submit_request(&mut request)?;

        if !request.is_successful() {
            error!("Read request failed: {}", request.status().description());
            return Err(VirtioError::BackendError);
        }

        debug!("Read completed successfully");
        Ok(request.data)
    }

    /// Write sectors to the block device
    pub fn write_sectors(
        &mut self,
        sector: u64,
        data: &[u8],
    ) -> VirtioResult<()> {
        if sector + (data.len() / self.config.blk_size as usize) as u64 > self.config.capacity {
            error!(
                "Write beyond device capacity: sector {} + {} > {}",
                sector,
                data.len() / self.config.blk_size as usize,
                self.config.capacity
            );
            return Err(VirtioError::InvalidSector);
        }

        debug!("Writing {} bytes to sector {}", data.len(), sector);

        // Create a write request
        let mut request = VirtioBlkRequest::new_write(sector, data.to_vec());

        // Submit the request and wait for completion
        self.submit_request(&mut request)?;

        if !request.is_successful() {
            error!("Write request failed: {}", request.status().description());
            return Err(VirtioError::BackendError);
        }

        debug!("Write completed successfully");
        Ok(())
    }

    /// Flush the block device
    pub fn flush(&mut self) -> VirtioResult<()> {
        debug!("Flushing block device");
        // Create a flush request
        let mut request = VirtioBlkRequest::new_flush();
        // Submit the request and wait for completion
        self.submit_request(&mut request)?;
        if !request.is_successful() {
            error!("Flush request failed: {}", request.status().description());
            return Err(VirtioError::BackendError);
        }
        debug!("Flush completed successfully");
        Ok(())
    }

    /// Submit a VirtIO Block request
    fn submit_request(&mut self, request: &mut VirtioBlkRequest) -> VirtioResult<()> {
        // Allocate descriptors for the request
        let mut desc_chain = VirtioBlkDescriptorChain::new();

        // Allocate descriptor for request header
        let header_desc = self.allocate_descriptor()?;
        self.virtqueue.queue_mut().descriptors[header_desc as usize] = Descriptor::new(
            &request.header as *const _ as u64,
            core::mem::size_of::<VirtioBlkReqHeader>() as u32,
            descriptor_flags::NEXT,
            0, // Will be set later
        );
        desc_chain.add_descriptor(
            header_desc,
            core::mem::size_of::<VirtioBlkReqHeader>() as u32,
        );

        // Allocate descriptor for data buffer (if any)
        let data_desc = if !request.data.is_empty() {
            let desc = self.allocate_descriptor()?;
            let flags = if request.is_read() {
                descriptor_flags::WRITE | descriptor_flags::NEXT
            } else {
                descriptor_flags::NEXT
            };

            self.virtqueue.queue_mut().descriptors[desc as usize] = Descriptor::new(
                request.data.as_ptr() as u64,
                request.data.len() as u32,
                flags,
                0, // Will be set later
            );
            desc_chain.add_descriptor(desc, request.data.len() as u32);
            Some(desc)
        } else {
            None
        };

        // Allocate descriptor for status
        let status_desc = self.allocate_descriptor()?;
        self.virtqueue.queue_mut().descriptors[status_desc as usize] = Descriptor::new(
            &request.status as *const _ as u64,
            core::mem::size_of::<VirtioBlkReqStatus>() as u32,
            descriptor_flags::WRITE,
            0,
        );
        desc_chain.add_descriptor(
            status_desc,
            core::mem::size_of::<VirtioBlkReqStatus>() as u32,
        );

        // Link the descriptors
        self.virtqueue.queue_mut().descriptors[header_desc as usize].next =
            if let Some(data_desc) = data_desc {
                data_desc
            } else {
                status_desc
            };

        if let Some(data_desc) = data_desc {
            self.virtqueue.queue_mut().descriptors[data_desc as usize].next = status_desc;
        }

        // Add to available ring
        let avail_idx = self.virtqueue.queue_mut().available.idx as usize
            % self.virtqueue.queue_mut().available.ring.len();
        self.virtqueue.queue_mut().available.ring[avail_idx] = header_desc;

        // Memory barrier
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        // Update available index
        self.virtqueue.queue_mut().available.idx =
            self.virtqueue.queue_mut().available.idx.wrapping_add(1);

        // Notify device
        write_mmio_u32(self.base_addr, mmio::QUEUE_NOTIFY, 0);

        // Wait for completion
        self.wait_for_completion(&desc_chain)?;

        // Free descriptors
        self.free_descriptor(header_desc);
        if let Some(data_desc) = data_desc {
            self.free_descriptor(data_desc);
        }
        self.free_descriptor(status_desc);

        Ok(())
    }

    /// Wait for request completion
    fn wait_for_completion(&mut self, desc_chain: &VirtioBlkDescriptorChain) -> VirtioResult<()> {
        let head_desc = desc_chain.head().ok_or(VirtioError::InvalidDescriptor)?;

        // Simple polling implementation
        // In a real implementation, this would use interrupts
        let mut timeout = 1000000; // Arbitrary timeout

        while timeout > 0 {
            // Check if there are any used descriptors
            if self.virtqueue.queue_mut().used.idx != self.virtqueue.queue_mut().last_used_idx {
                // Process used descriptors
                while self.virtqueue.queue_mut().last_used_idx
                    != self.virtqueue.queue_mut().used.idx
                {
                    let used_idx = self.virtqueue.queue_mut().last_used_idx as usize
                        % self.virtqueue.queue_mut().used.ring.len();
                    let used_elem = &self.virtqueue.queue_mut().used.ring[used_idx];

                    if used_elem.id == head_desc as u32 {
                        debug!("Request completed with length: {}", used_elem.len);
                        self.virtqueue.queue_mut().last_used_idx =
                            self.virtqueue.queue_mut().last_used_idx.wrapping_add(1);
                        return Ok(());
                    }

                    self.virtqueue.queue_mut().last_used_idx =
                        self.virtqueue.queue_mut().last_used_idx.wrapping_add(1);
                }
            }

            timeout -= 1;
            // Small delay
            for _ in 0..1000 {
                core::hint::spin_loop();
            }
        }

        error!("Request timed out");
        Err(VirtioError::BackendError)
    }

    /// Allocate a descriptor from the free list
    fn allocate_descriptor(&mut self) -> VirtioResult<u16> {
        self.virtqueue
            .queue_mut()
            .free_descriptors
            .pop()
            .ok_or(VirtioError::InvalidDescriptor)
    }

    /// Free a descriptor back to the free list
    fn free_descriptor(&mut self, desc_idx: u16) {
        self.virtqueue.queue_mut().free_descriptors.push(desc_idx);
    }
}
