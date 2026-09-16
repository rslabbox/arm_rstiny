//! Device discovery and input: probe the granted MMIO window for the display
//! and every virtio-input device, and pack input events for INPUT_READ.

use core::ptr::NonNull;

use virtio_drivers::{
    device::gpu::VirtIOGpu,
    device::input::VirtIOInput,
    transport::{DeviceType, Transport, mmio::MmioTransport},
};

use crate::hal::HalImpl;

pub const SLOT_STRIDE: u64 = 0x200;

pub fn probe_gpu(
    window: usize,
    window_size: usize,
) -> Option<VirtIOGpu<HalImpl, MmioTransport<'static>>> {
    let slots = window_size / SLOT_STRIDE as usize;
    for slot in 0..slots {
        // SAFETY: the window is a device Untyped frame range this task mapped
        // exclusively; every 0x200 slot is a valid VirtIO MMIO region.
        let Ok(transport) = (unsafe {
            MmioTransport::new(
                NonNull::new((window + slot * SLOT_STRIDE as usize) as *mut _)?,
                SLOT_STRIDE as usize,
            )
        }) else {
            continue;
        };
        if transport.device_type() != DeviceType::GPU {
            // MmioTransport's Drop resets the device: forget it so a live
            // device (block-server's, or our own after a re-probe) is left
            // untouched. The transport owns no allocation.
            core::mem::forget(transport);
            continue;
        }
        return VirtIOGpu::new(transport).ok();
    }
    None
}

/// Probe every slot in the granted MMIO window and collect every
/// virtio-input device (keyboard, mouse, ...).

pub fn probe_inputs(
    window: usize,
    window_size: usize,
) -> alloc::vec::Vec<VirtIOInput<HalImpl, MmioTransport<'static>>> {
    let slots = window_size / SLOT_STRIDE as usize;
    let mut inputs = alloc::vec::Vec::new();
    for slot in 0..slots {
        // SAFETY: as `probe_gpu`: exclusively mapped device window.
        let Some(base) = NonNull::new((window + slot * SLOT_STRIDE as usize) as *mut _) else {
            continue;
        };
        let transport = match unsafe { MmioTransport::new(base, SLOT_STRIDE as usize) } {
            Ok(transport) => transport,
            Err(_) => continue,
        };
        if transport.device_type() != DeviceType::Input {
            // MmioTransport's Drop resets the device: forget it, or scanning
            // past the inputs wipes the already-initialised GPU and the
            // block-server's device (the six-service boot deadlock). The
            // transport owns no allocation.
            core::mem::forget(transport);
            continue;
        }
        if let Ok(device) = VirtIOInput::new(transport) {
            inputs.push(device);
        }
    }
    inputs
}
