// ============================================================================
// DesignWare PCIe ATU (Address Translation Unit) Support
// ============================================================================

use core::time::Duration;

use tock_registers::{
    interfaces::{Readable, Writeable},
    register_structs,
    registers::ReadWrite,
};

use crate::TinyResult;
use crate::error::TinyError;

register_structs! {
    /// iATU Unroll mode register layout
    #[allow(non_snake_case)]
    pub AtuRegion {
        (0x00 => REGION_CTRL1: ReadWrite<u32>),
        (0x04 => REGION_CTRL2: ReadWrite<u32>),
        (0x08 => LOWER_BASE: ReadWrite<u32>),
        (0x0C => UPPER_BASE: ReadWrite<u32>),
        (0x10 => LOWER_LIMIT: ReadWrite<u32>),
        (0x14 => UPPER_LIMIT: ReadWrite<u32>),
        (0x18 => LOWER_TARGET: ReadWrite<u32>),
        (0x1C => UPPER_TARGET: ReadWrite<u32>),
        (0x20 => @END),
    }
}

/// iATU Unroll base address offset (DBI + 0x300000)
const DEFAULT_DBI_ATU_OFFSET: u64 = 0x3 << 20; // 0x300000

/// iATU Enable bit
const PCIE_ATU_ENABLE: u32 = 1 << 31;

/// iATU Transaction Type
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum AtuType {
    /// Memory transaction
    Memory = 0x0,
    /// I/O transaction
    Io = 0x2,
    /// Configuration Type 0
    Config0 = 0x4,
    /// Configuration Type 1
    Config1 = 0x5,
}

impl AtuType {
    /// Convert to u32 value
    #[inline]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }
}

/// iATU Region Index
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum AtuRegionIndex {
    /// Region 0
    Region0 = 0,
    /// Region 1
    Region1 = 1,
}

impl AtuRegionIndex {
    /// Convert to u32 value
    #[inline]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }
}

/// Maximum retries for iATU enable
const LINK_WAIT_MAX_IATU_RETRIES: usize = 5;

/// Get outbound iATU region register offset
/// Each region is 512 bytes (region << 9)
#[inline]
const fn get_atu_outb_unr_reg_offset(region: u32) -> usize {
    (region as usize) << 9
}

/// DesignWare PCIe controller ATU operations
#[derive(Debug, Clone)]
pub struct DwPcieAtu {
    atu_base_virt: usize,
}

impl DwPcieAtu {
    /// Create new ATU accessor
    pub fn new(dbi_base_virt: usize) -> Self {
        let atu_base_virt = dbi_base_virt + (DEFAULT_DBI_ATU_OFFSET as usize);

        debug!("DesignWare PCIe ATU Initialization:");
        debug!("  DBI Base:  {:#018x}", dbi_base_virt);
        debug!("  ATU Base:  virt={:#018x}", atu_base_virt);

        Self { atu_base_virt }
    }

    /// Get reference to ATU region registers
    #[inline]
    fn get_region(&self, index: u32) -> &AtuRegion {
        let offset = get_atu_outb_unr_reg_offset(index);
        let addr = (self.atu_base_virt + offset) as *const AtuRegion;
        unsafe { &*addr }
    }

    /// Program outbound ATU (Unroll mode)
    ///
    /// # Parameters
    /// * `index` - ATU region index (AtuRegionIndex enum)
    /// * `atu_type` - ATU access type (AtuType enum)
    /// * `cpu_addr` - Physical address for the translation entry (source)
    /// * `pci_addr` - PCIe bus address for the translation entry (target)
    /// * `size` - Size of the translation entry
    ///
    /// # Returns
    /// * `Ok(())` on success
    /// * `Err(TinyError)` on failure
    pub fn prog_outbound_atu(
        &self,
        index: AtuRegionIndex,
        atu_type: AtuType,
        cpu_addr: u64,
        pci_addr: u64,
        size: u32,
    ) -> TinyResult<()> {
        info!(
            "ATU[{}]: type={:#x}, cpu={:#016x}, pci={:#016x}, size={:#x}",
            index.as_u32(),
            atu_type.as_u32(),
            cpu_addr,
            pci_addr,
            size
        );

        let region = self.get_region(index.as_u32());

        // Configure lower and upper base (source CPU address)
        let lower_base = (cpu_addr & 0xFFFFFFFF) as u32;
        let upper_base = (cpu_addr >> 32) as u32;

        region.LOWER_BASE.set(lower_base);
        region.UPPER_BASE.set(upper_base);

        // Configure limit (end of source address range)
        let limit_addr = cpu_addr + (size as u64) - 1;
        let lower_limit = (limit_addr & 0xFFFFFFFF) as u32;
        let upper_limit = (limit_addr >> 32) as u32;

        region.LOWER_LIMIT.set(lower_limit);
        region.UPPER_LIMIT.set(upper_limit);

        // Configure target address (PCIe bus address)
        let lower_target = (pci_addr & 0xFFFFFFFF) as u32;
        let upper_target = (pci_addr >> 32) as u32;

        region.LOWER_TARGET.set(lower_target);
        region.UPPER_TARGET.set(upper_target);

        // Configure region control (transaction type)
        region.REGION_CTRL1.set(atu_type.as_u32());

        // Enable ATU region
        region.REGION_CTRL2.set(PCIE_ATU_ENABLE);

        // Wait for ATU enable to take effect
        for retry in 0..LINK_WAIT_MAX_IATU_RETRIES {
            let val = region.REGION_CTRL2.get();
            if (val & PCIE_ATU_ENABLE) != 0 {
                if retry > 0 {
                    debug!("ATU[{}] enabled after {} retries", index.as_u32(), retry);
                }
                return Ok(());
            }
            crate::drivers::timer::busy_wait(Duration::from_millis(1));
        }

        error!("ATU[{}] enable timeout!", index.as_u32());
        Err(TinyError::PcieAtuNotEnabled)
    }

    /// Dump ATU region configuration for debugging
    pub fn dump_atu_config(&self, index: AtuRegionIndex) {
        let region = self.get_region(index.as_u32());

        let lower_base = region.LOWER_BASE.get();
        let upper_base = region.UPPER_BASE.get();
        let lower_limit = region.LOWER_LIMIT.get();
        let upper_limit = region.UPPER_LIMIT.get();
        let lower_target = region.LOWER_TARGET.get();
        let upper_target = region.UPPER_TARGET.get();
        let ctrl1 = region.REGION_CTRL1.get();
        let ctrl2 = region.REGION_CTRL2.get();

        info!("ATU Region {} Configuration:", index.as_u32());
        info!("  CTRL1 (Type):     {:#010x}", ctrl1);
        info!(
            "  CTRL2 (Enable):   {:#010x} {}",
            ctrl2,
            if (ctrl2 & PCIE_ATU_ENABLE) != 0 {
                "[ENABLED]"
            } else {
                "[DISABLED]"
            }
        );
        info!(
            "  Source (CPU):     {:#010x}_{:08x}",
            upper_base, lower_base
        );
        info!(
            "  Limit:            {:#010x}_{:08x}",
            upper_limit, lower_limit
        );
        info!(
            "  Target (PCIe):    {:#010x}_{:08x}",
            upper_target, lower_target
        );
    }
}
