use memory_addr::PhysAddr;

use crate::TinyResult;
use crate::mm::phys_to_virt;

mod atu;

pub struct DwPcie {
    atu: atu::DwPcieAtu,
    cpu_addr: usize,
    pci_addr: usize,
    size: usize,
}

impl DwPcie {
    pub fn new(dbi_base_virt: usize) -> Self {
        let atu = atu::DwPcieAtu::new(dbi_base_virt);
        Self {
            atu,
            cpu_addr: 0xf300_0000usize,
            pci_addr: 0x0usize,
            size: 0x10_0000usize,
        }
    }

    pub fn pcie_dw_read_config(&self, addr: u64) -> TinyResult<u32> {
        unsafe {
            let virt_addr = phys_to_virt(PhysAddr::from(addr as usize));
            let val = core::ptr::read_volatile(virt_addr.as_ptr() as *const u32);
            Ok(val)
        }
    }
}

/// DBI (DesignWare Bus Interface) register base address for RK3588
#[allow(dead_code)]
const DBI_BASE: u64 = 0xa40c00000;

/// Test DesignWare PCIe ATU functionality
///
/// This function demonstrates how to use the ATU to configure outbound
/// address translation for PCIe configuration space access.
pub fn test_dw_pcie_atu() {
    info!("=== Testing DesignWare PCIe ATU ===");

    let dbi_base = phys_to_virt(PhysAddr::from(DBI_BASE as usize)).as_usize();
    let atu = atu::DwPcieAtu::new(dbi_base);

    // Example: Configure ATU for configuration space access
    // Region 1, Type CFG0, CPU address -> PCIe bus address
    let cpu_addr = 0xf300_0000u64; // Configuration window
    let pci_addr = 0x0000_0000u64; // Bus 0, Device 0, Function 0
    let size = 0x10_0000u32; // 1MB window

    info!("Configuring ATU region 1 for configuration access");
    match atu.prog_outbound_atu(
        atu::AtuRegionIndex::Region1,
        atu::AtuType::Config0,
        cpu_addr,
        pci_addr,
        size,
    ) {
        Ok(_) => {
            info!("ATU configuration successful");
            atu.dump_atu_config(atu::AtuRegionIndex::Region1);

            // READ pci test 0xf3000000
            let test_addr = phys_to_virt(PhysAddr::from(0xf300_0000u64 as usize));
            unsafe {
                let val = core::ptr::read_volatile(test_addr.as_ptr() as *const u32);
                info!("Read from PCI config space at 0xf3000000: {:#010x}", val);
            }
        }
        Err(e) => {
            error!("ATU configuration failed: {}", e);
        }
    }
}
