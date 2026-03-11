//! Device drivers module.
//!
//! This module contains drivers for various hardware devices organized by category.

use crate::device::core::Bus;

pub mod fdt;
pub mod irq;
pub mod power;
pub mod timer;
pub mod uart;
pub mod virtio;

pub fn driver_init_early() {
    let early_bus = crate::device::core::EarlyBus;
    for level in [
        crate::device::core::InitLevel::Early,
        crate::device::core::InitLevel::Core,
    ] {
        early_bus.for_each_device(|dev| {
            crate::device::core::driver_manager().bind_device_for_level(&dev, level);
        });
    }
}

pub fn driver_init() {
    let bus = crate::device::core::FdtBus;
    for level in [
        crate::device::core::InitLevel::Normal,
        crate::device::core::InitLevel::Late,
    ] {
        bus.for_each_device(|dev| {
            crate::device::core::driver_manager().bind_device_for_level(&dev, level);
        });
    }
}
