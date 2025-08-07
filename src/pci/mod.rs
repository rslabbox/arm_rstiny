use core::{any::Any, ffi::CStr, ptr::NonNull};

use crate::utils::heap_allocator::PAGE_SIZE;
use alloc::{ffi::CString, format, string::String};
use axdriver_pci::{
    BarInfo, Cam, Command, DeviceFunction, HeaderType, MemoryBarType, PciRangeAllocator, PciRoot,
};
use byte_unit::Byte;
use dma_api::{Direction, set_impl};
use nvme_driver::{Config, Nvme};

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

// NVMe controller PCI class codes
const PCI_CLASS_STORAGE: u8 = 0x01;
const PCI_SUBCLASS_NVM: u8 = 0x08;
const PCI_PROG_IF_NVME: u8 = 0x02;

/// Simple memory mapping function for PCI BAR addresses
/// In a real implementation, this would set up proper virtual memory mapping
fn simple_iomap(phys_addr: u64, _size: usize) -> NonNull<u8> {
    // For now, we assume identity mapping (physical == virtual)
    // In a real OS, this would involve setting up page tables
    NonNull::new(phys_addr as *mut u8).expect("Invalid physical address")
}

// DMA API implementation for NVMe driver
struct DmaImpl;

impl dma_api::Impl for DmaImpl {
    fn map(addr: NonNull<u8>, _size: usize, _direction: Direction) -> u64 {
        // For identity mapping, physical address equals virtual address
        addr.as_ptr() as u64
    }

    fn unmap(_addr: NonNull<u8>, _size: usize) {
        // No-op for identity mapping
    }

    fn flush(_addr: NonNull<u8>, _size: usize) {
        // No-op for now - in a real implementation, this would flush CPU caches
    }

    fn invalidate(_addr: NonNull<u8>, _size: usize) {
        // No-op for now - in a real implementation, this would invalidate CPU caches
    }
}

// DMA API implementation is set up using the set_impl! macro
// But we need to provide the alloc/dealloc functions manually
#[unsafe(no_mangle)]
extern "Rust" fn __dma_api_alloc(layout: core::alloc::Layout) -> *mut u8 {
    unsafe { alloc::alloc::alloc(layout) }
}

#[unsafe(no_mangle)]
extern "Rust" fn __dma_api_dealloc(ptr: *mut u8, layout: core::alloc::Layout) {
    unsafe { alloc::alloc::dealloc(ptr, layout) }
}

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

fn nvme_pci_read_test(nvme: &mut Nvme) {
    let namespace_list = nvme
        .namespace_list()
        .inspect_err(|e| error!("{e:?}"))
        .unwrap();
    for ns in &namespace_list {
        let space = Byte::from_u64(ns.lba_size as u64 * ns.lba_count as u64);

        info!("namespace: {:?}, space: {:#}", ns, space);
    }

            for _i in 0..128 {
            let _ = nvme
                .namespace_list()
                .inspect_err(|e| error!("{e:?}"))
                .unwrap();
        }

        info!("admin queue test ok");

        let ns = namespace_list[0];

        for i in 0..128 {
            let want_str = format!("hello world! block {i}");

            let want = CString::new(want_str.as_str()).unwrap();

            let want_bytes = want.to_bytes();

            // buff 大小需与块大小一致
            let mut write_buff = alloc::vec![0u8; ns.lba_size];

            write_buff[0..want_bytes.len()].copy_from_slice(want_bytes);

            nvme.block_write_sync(&ns, i, &write_buff).unwrap();

            let mut buff = alloc::vec![0u8; ns.lba_size];

            nvme.block_read_sync(&ns, i, &mut buff).unwrap();

            let read_result = unsafe { CStr::from_ptr(buff.as_ptr() as _) }.to_str();

            info!("read result: {:?}", read_result.unwrap());

            assert_eq!(Ok(want_str.as_str()), read_result);
        }

        info!("test passed!");
}

pub fn nvme_pci_test() {
    // Set up DMA API implementation
    set_impl!(DmaImpl);

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

            // Check if this is an NVMe controller
            let is_nvme = dev_info.class == PCI_CLASS_STORAGE
                && dev_info.subclass == PCI_SUBCLASS_NVM
                && dev_info.prog_if == PCI_PROG_IF_NVME;

            match config_pci_device(&mut root, bdf, &mut allocator) {
                Ok(_) => {
                    info!("PCI device {} enabled", bdf);

                    // If this is an NVMe controller, try to instantiate it
                    if is_nvme {
                        info!("Found NVMe controller at {}", bdf);

                        // Get BAR0 information for NVMe controller
                        if let Ok(bar_info) = root.bar_info(bdf, 0) {
                            if let BarInfo::Memory { address, size, .. } = bar_info {
                                if address > 0 && size > 0 {
                                    info!("NVMe BAR0: address={:#x}, size={:#x}", address, size);

                                    // Map the BAR address
                                    let bar_ptr = simple_iomap(address, size as usize);

                                    // Create NVMe configuration
                                    let config = Config {
                                        page_size: PAGE_SIZE,
                                        io_queue_pair_count: 1,
                                    };

                                    // Try to instantiate the NVMe device
                                    match Nvme::new(bar_ptr, config) {
                                        Ok(mut nvme) => {
                                            info!("NVMe device instantiated successfully!");
                                            // Note: We're not storing the nvme instance as requested
                                            // In a real implementation, you would store it somewhere
                                            nvme_pci_read_test(&mut nvme);
                                        }
                                        Err(e) => {
                                            error!("Failed to instantiate NVMe device: {:?}", e);
                                        }
                                    }
                                } else {
                                    warn!("NVMe controller BAR0 not properly configured");
                                }
                            } else {
                                warn!("NVMe controller BAR0 is not a memory BAR");
                            }
                        } else {
                            error!("Failed to get BAR info for NVMe controller");
                        }
                    }
                }
                Err(e) => warn!(
                    "failed to enable PCI device at {}({}): {:?}",
                    bdf, dev_info, e
                ),
            }
        }
    }
}
