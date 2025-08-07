mod device_type;

use crate::virtio::constants::VIRTIO_MMIO_MAGIC;
pub use device_type::VirtioDeviceID;
use log::info;

const VIRTIO_MMIO_BASE: usize = 0x0A00_0000; // VirtIO MMIO devices start at this address

pub fn virtio_discover_device(device_type: VirtioDeviceID) -> Option<usize> {
    info!("Scanning for VirtIO devices...");

    // VirtIO MMIO 设备通常在 0x0a000000 开始，每个设备占用 0x200 字节
    for device_index in 0..32 {
        let device_addr = VIRTIO_MMIO_BASE + (device_index * 0x200);

        // 尝试读取魔数来检查是否有设备
        let magic = crate::virtio::mmio::read_mmio_u32(device_addr, 0x000);
        if magic == VIRTIO_MMIO_MAGIC {
            // "virt"
            let device_id = crate::virtio::mmio::read_mmio_u32(device_addr, 0x008);

            if device_id == device_type.to_device_id() {
                info!("Found VirtIO Block device at 0x{:x}", device_addr);
                return Some(device_addr);
            }
        }
    }
    None
}
