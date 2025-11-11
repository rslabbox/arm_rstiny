use core::error;

use memory_addr::PhysAddr;

use crate::{drivers::pci::realtek::NetDriverOps, mm::phys_to_virt};

mod atu;
mod bus;
pub mod realtek;

/// DBI (DesignWare Bus Interface) register base address for RK3588
#[allow(dead_code)]
const DBI_BASE: u64 = 0xa40c00000;

/// Test DesignWare PCIe ATU functionality
///
/// This function demonstrates how to use the ATU to configure outbound
/// address translation for PCIe configuration space access.
pub fn test_dw_pcie_atu() {
    info!("=== Testing DesignWare PCIe ATU ===");
    let mmio_base = phys_to_virt(PhysAddr::from(0xf300_0000usize));
    let dbi_base = phys_to_virt(PhysAddr::from(DBI_BASE as usize));
    let cpu_addr = 0xf300_0000usize; // Configuration window
    let pci_addr = 0x0000_0000usize; // Bus 0, Device 0, Function 0
    let phy_addr = 0x40100000usize; // Physical start address
    let size = 0x10_0000usize; // 1MB window
    let mut root = unsafe {
        bus::PciRoot::new(
            mmio_base.as_mut_ptr(),
            dbi_base.as_usize(),
            cpu_addr,
            pci_addr,
            size,
            phy_addr,
            bus::Cam::MmioCam,
        )
    };

    let (bdf, dev_info) = root
        .enumerate_bus(0)
        .next()
        .expect("Failed to enumerate PCIe bus");

    info!("PCI {}: {}", bdf, dev_info);

    if !realtek::is_realtek_device(dev_info.vendor_id, dev_info.device_id) {
        error!("No RealTek device found for testing.");
        return;
    }

    let bar_info = root.bar_info(bdf, 2).unwrap();

    info!("RealTek device BAR{} info: {:?}", 0, bar_info);
    match bar_info {
        bus::BarInfo::Memory {
            address, size: _, ..
        } => {
            info!("Mapping RealTek BAR{} at Memory address {:#x}", 2, address);

            let mmio_vaddr = crate::mm::phys_to_virt((0x9c0100000 as usize).into()).as_usize();

            // 直接从 mmio_vaddr 读取一个 u32
            let value = unsafe { core::ptr::read_volatile(mmio_vaddr as *const u32) };
            info!("Read value from I/O BAR address {:#x}: {:#x}", address, value);

            if let Ok(mut realtek) =
                realtek::create_driver(dev_info.vendor_id, dev_info.device_id, mmio_vaddr, 0xea)
            {
                let mac = realtek.mac_address();
                info!(
                    "RealTek device MAC address: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    mac.0[0], mac.0[1], mac.0[2], mac.0[3], mac.0[4], mac.0[5]
                );

                // Test ping functionality (RTL8169 only)
                if let realtek::RealtekDriverNic::Rtl8169(ref mut driver) = realtek {
                    crate::net::test_ping(driver);
                } else {
                    warn!("Ping test only supports RTL8169, skipping for RTL8139");
                }
            } else {
                error!("Failed to create RealTek driver instance for testing.");
            }
        }
        bus::BarInfo::IO { address, .. } => {
            error!("realtek: BAR{} is of I/O type, address {:#x}", 2, address);

            return;
        }
    }
}
