//! Common traits and types for network device (NIC) drivers.

mod driver_base;
mod net_buf;
mod smoltcp_impl;

/// PCI device probing and configuration (legacy ECAM).
pub mod pci;

/// DesignWare PCIe controller driver with iATU support.
pub mod pcie_dw;

/// RealTek RTL8139/RTL8169/RTL8168/RTL8111 NIC device driver.
pub mod realtek;

pub use net_buf::{NetBuf, NetBufBox, NetBufPool};
pub use smoltcp_impl::SmoltcpDevice;

#[doc(no_inline)]
pub use driver_base::*;

pub fn probe_mmio_device() {
    info!("=== Starting PCIe Device Probing (Using DW iATU) ===");

    pci::test_dw_pcie_atu();

    loop {}

    // First test if iATU is working properly
    pcie_dw::test_iatu();

    // Scan PCIe bus
    let devices = pcie_dw::scan_pcie_devices();

    if devices.is_empty() {
        warn!("No PCIe devices found!");
        return;
    }

    // Find network controllers
    let network_controllers: alloc::vec::Vec<_> = devices
        .iter()
        .filter(|d| d.is_network_controller())
        .collect();

    if !network_controllers.is_empty() {
        info!("Found {} Network Controller(s):", network_controllers.len());
        for nc in &network_controllers {
            info!(
                "  [{}:{:02x}.{}] {:04x}:{:04x} - {}",
                nc.bus,
                nc.dev,
                nc.func,
                nc.vendor_id,
                nc.device_id,
                nc.device_type_name()
            );

            // Print BARs
            for (i, bar) in nc.bars.iter().enumerate() {
                if *bar != 0 {
                    info!("    BAR{}: {:#010x}", i, bar);
                }
            }
        }
    }

    // Find Realtek RTL8125 NIC
    info!("\n=== Searching for Realtek RTL8125 NIC ===");
    for device in &devices {
        if device.vendor_id == 0x10ec && device.device_id == 0x8125 {
            info!("Found Realtek RTL8125 NIC!");
            info!(
                "  Location: Bus {:02x}, Dev {:02x}, Func {:x}",
                device.bus, device.dev, device.func
            );

            // Get MMIO base address (usually BAR0 or BAR2)
            let mut mmio_base: Option<usize> = None;
            for (i, bar) in device.bars.iter().enumerate() {
                if *bar != 0 && (*bar & 0x1) == 0 {
                    // Memory BAR
                    let bar_addr = (*bar & !0xF) as usize;
                    info!("  BAR{}: {:#010x}", i, bar_addr);

                    if mmio_base.is_none() {
                        mmio_base = Some(bar_addr);
                    }
                }
            }

            if let Some(phys_addr) = mmio_base {
                info!("  MMIO Physical Address: {:#010x}", phys_addr);

                let virt_addr =
                    crate::arch::mem::phys_to_virt(memory_addr::PhysAddr::from(phys_addr))
                        .as_usize();
                info!("  MMIO Virtual Address: {:#018x}", virt_addr);

                // Try to create driver
                match realtek::create_driver(
                    device.vendor_id,
                    device.device_id,
                    phys_addr,
                    0xea, // IRQ (get from device tree or config space)
                ) {
                    Ok(mut net_dev) => {
                        let mac = net_dev.mac_address();
                        info!("  Driver Initialized Successfully!");
                        info!("  MAC Address: {:?}", mac);

                        // Print driver status
                        info!("  can_receive: {}", net_dev.can_receive());
                        info!("  can_transmit: {}", net_dev.can_transmit());

                        // Try to receive packets
                        info!("\n  Trying to receive packets (polling 5 times)...");
                        for i in 0..5 {
                            match net_dev.receive() {
                                Ok(buf) => {
                                    info!(
                                        "    [{}] Received packet: {} bytes",
                                        i,
                                        buf.packet_len()
                                    );
                                    let _ = net_dev.recycle_rx_buffer(buf);
                                }
                                Err(e) => {
                                    if i == 0 {
                                        debug!("    [{}] No packet available: {:?}", i, e);
                                    }
                                }
                            }
                            // Short delay
                            for _ in 0..100000 {
                                core::hint::spin_loop();
                            }
                        }

                        // Start network test
                        info!("\n=== Starting Network Stack Test ===");
                        crate::net::ping_test(
                            net_dev,
                            smoltcp::wire::Ipv4Address::new(10, 19, 0, 1),
                        );

                        return;
                    }
                    Err(e) => {
                        error!("  Driver Initialization Failed: {:?}", e);
                    }
                }
            } else {
                warn!("  No valid MMIO BAR found");
            }

            break;
        }
    }

    warn!("Realtek RTL8125 NIC not found, using hardcoded address for testing...");

    // Fallback to hardcoded address
    const REALTEK_BASE: usize = 0xf3100000;
    const VENDOR_ID: u16 = 0x10EC;
    const DEVICE_ID: u16 = 0x8125;

    let rtl8125_vaddr =
        crate::arch::mem::phys_to_virt(memory_addr::PhysAddr::from(REALTEK_BASE)).as_usize();

    info!(
        "Using hardcoded address: phys={:#x}, virt={:#x}",
        REALTEK_BASE, rtl8125_vaddr
    );

    match realtek::create_driver(VENDOR_ID, DEVICE_ID, REALTEK_BASE, 0xea) {
        Ok(net_dev) => {
            let mac = net_dev.mac_address();
            info!("Detected Realtek RTL8125 NIC, MAC: {:?}", mac);

            // Start network test
            crate::net::ping_test(net_dev, smoltcp::wire::Ipv4Address::new(10, 19, 0, 1));
        }
        Err(e) => {
            error!("Driver initialization failed: {:?}", e);
        }
    }
}
