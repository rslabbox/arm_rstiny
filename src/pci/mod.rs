use core::any::Any;

use alloc::string::String;
use axdriver_pci::{
    BarInfo, Cam, Command, DeviceFunction, HeaderType, MemoryBarType, PciRangeAllocator, PciRoot,
};

// # Base physical address of the PCIe ECAM space.
// pci-ecam-base = 0x40_1000_0000  # uint
// # End PCI bus number (`bus-range` property in device tree).
// pci-bus-end = 0xff              # uint
// # PCI device memory ranges (`ranges` property in device tree).
const PCI_RANGES: [[u64; 2]; 3] = [
    [0x3ef_f0000, 0x1_0000],          // PIO space
    [0x1000_0000, 0x2eff_0000],       // 32-bit MMIO space
    [0x80_0000_0000, 0x80_0000_0000], // 64-bit MMIO space
]; // [(uint, uint)]

const PCI_BUS_END: u8 = 0xff;
const PCI_BAR_NUM: u8 = 6;

fn config_pci_device(
    root: &mut PciRoot,
    bdf: DeviceFunction,
    allocator: &mut Option<PciRangeAllocator>,
) -> Result<(), String> {
    let mut bar = 0;
    while bar < PCI_BAR_NUM {
        let info = root.bar_info(bdf, bar).unwrap();
        if let BarInfo::Memory {
            address_type,
            address,
            size,
            ..
        } = info
        {
            // if the BAR address is not assigned, call the allocator and assign it.
            if size > 0 && address == 0 {
                let new_addr = allocator
                    .as_mut()
                    .expect("No memory ranges available for PCI BARs!")
                    .alloc(size as _)
                    .ok_or("Failed to allocate memory for PCI BAR")?;
                if address_type == MemoryBarType::Width32 {
                    root.set_bar_32(bdf, bar, new_addr as _);
                } else if address_type == MemoryBarType::Width64 {
                    root.set_bar_64(bdf, bar, new_addr);
                }
            }
        }

        // read the BAR info again after assignment.
        let info = root.bar_info(bdf, bar).unwrap();
        match info {
            BarInfo::IO { address, size } => {
                if address > 0 && size > 0 {
                    debug!("  BAR {}: IO  [{:#x}, {:#x})", bar, address, address + size);
                }
            }
            BarInfo::Memory {
                address_type,
                prefetchable,
                address,
                size,
            } => {
                if address > 0 && size > 0 {
                    debug!(
                        "  BAR {}: MEM [{:#x}, {:#x}){}{}",
                        bar,
                        address,
                        address + size as u64,
                        if address_type == MemoryBarType::Width64 {
                            " 64bit"
                        } else {
                            ""
                        },
                        if prefetchable { " pref" } else { "" },
                    );
                }
            }
        }

        bar += 1;
        if info.takes_two_entries() {
            bar += 1;
        }
    }

    // Enable the device.
    let (_status, cmd) = root.get_status_command(bdf);
    root.set_command(
        bdf,
        cmd | Command::IO_SPACE | Command::MEMORY_SPACE | Command::BUS_MASTER,
    );
    Ok(())
}

pub fn nvme_pci_test() {
    let base_addr = 0x40_1000_0000 as *mut u8;
    let mut root = unsafe { PciRoot::new(base_addr, Cam::Ecam) };

    let mut allocator = PCI_RANGES
        .get(1)
        .map(|range| PciRangeAllocator::new(range[0], range[1]));

    for bus in 0..=PCI_BUS_END as u8 {
        for (bdf, dev_info) in root.enumerate_bus(bus) {
            debug!("PCI {}: {} {:?}", bdf, dev_info, dev_info.type_id());
            if dev_info.header_type != HeaderType::Standard {
                continue;
            }
            match config_pci_device(&mut root, bdf, &mut allocator) {
                Ok(_) => info!("PCI device {} enabled", bdf),
                Err(e) => warn!(
                    "failed to enable PCI device at {}({}): {:?}",
                    bdf, dev_info, e
                ),
            }
        }
    }
}
