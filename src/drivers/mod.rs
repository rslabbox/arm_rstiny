//! Device drivers module.
//!
//! This module contains drivers for various hardware devices organized by category.

pub mod irq;
pub mod power;
pub mod timer;
pub mod uart;
pub mod virtio;
pub mod fdt;

use core::ptr::NonNull;

use virtio_drivers::{transport::{Transport, mmio::{MmioTransport, VirtIOHeader}}};

pub fn driver_init() {
    let fdt = crate::drivers::fdt::get_fdt().lock();
    for node in fdt.all_nodes() {
        // Check whether it is a VirtIO MMIO device.
        if let (Some(compatible), Some(region)) = (node.compatible(), node.reg().next()) {
            if compatible.all().any(|s| s == "virtio,mmio")
                && region.size.unwrap_or(0) > size_of::<VirtIOHeader>()
            {
                let header = NonNull::new(region.starting_address as *mut VirtIOHeader).unwrap();
                match unsafe { MmioTransport::new(header, region.size.unwrap()) } {
                    Err(_e) =>  {}, 
                    Ok(transport) => {
                        match transport.device_type() {
                            virtio_drivers::transport::DeviceType::Block => {
                                virtio::blk::init(transport);
                            }
                            _ => {
                                // Unsupported device type; ignore.
                            }
                        }
                    }
                }
            }
        }
    }
}

