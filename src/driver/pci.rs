//! PCI device probing and configuration.
//!
//! This module provides functionality to probe and configure PCI/PCIe devices
//! on ARM64 systems using memory-mapped configuration space (ECAM).

use crate::arch::mem::phys_to_virt;
use memory_addr::PhysAddr;

/// PCI Configuration Space Header Type 0 (Normal Device)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PciConfigHeader {
    pub vendor_id: u16,
    pub device_id: u16,
    pub command: u16,
    pub status: u16,
    pub revision_id: u8,
    pub prog_if: u8,
    pub subclass: u8,
    pub class_code: u8,
    pub cache_line_size: u8,
    pub latency_timer: u8,
    pub header_type: u8,
    pub bist: u8,
    pub bar: [u32; 6],
    pub cardbus_cis_ptr: u32,
    pub subsystem_vendor_id: u16,
    pub subsystem_id: u16,
    pub expansion_rom_base: u32,
    pub capabilities_ptr: u8,
    pub reserved: [u8; 7],
    pub interrupt_line: u8,
    pub interrupt_pin: u8,
    pub min_grant: u8,
    pub max_latency: u8,
}

/// PCI Configuration Space Header Type 1 (PCI-to-PCI Bridge)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PciBridgeHeader {
    pub vendor_id: u16,
    pub device_id: u16,
    pub command: u16,
    pub status: u16,
    pub revision_id: u8,
    pub prog_if: u8,
    pub subclass: u8,
    pub class_code: u8,
    pub cache_line_size: u8,
    pub latency_timer: u8,
    pub header_type: u8,
    pub bist: u8,
    pub bar: [u32; 2],
    pub primary_bus: u8,
    pub secondary_bus: u8,
    pub subordinate_bus: u8,
    pub secondary_latency_timer: u8,
    pub io_base: u8,
    pub io_limit: u8,
    pub secondary_status: u16,
    pub memory_base: u16,
    pub memory_limit: u16,
    pub prefetchable_memory_base: u16,
    pub prefetchable_memory_limit: u16,
    pub prefetchable_base_upper: u32,
    pub prefetchable_limit_upper: u32,
    pub io_base_upper: u16,
    pub io_limit_upper: u16,
    pub capabilities_ptr: u8,
    pub reserved: [u8; 3],
    pub expansion_rom_base: u32,
    pub interrupt_line: u8,
    pub interrupt_pin: u8,
    pub bridge_control: u16,
}

/// PCI device information
#[derive(Debug, Clone)]
pub struct PciDeviceInfo {
    /// Bus number
    pub bus: u8,
    /// Device number
    pub device: u8,
    /// Function number
    pub function: u8,
    /// Vendor ID
    pub vendor_id: u16,
    /// Device ID
    pub device_id: u16,
    /// Class code
    pub class_code: u8,
    /// Subclass
    pub subclass: u8,
    /// Programming interface
    pub prog_if: u8,
    /// Revision ID
    pub revision_id: u8,
    /// Base Address Registers
    pub bars: [u32; 6],
    /// Interrupt line
    pub interrupt_line: u8,
    /// Interrupt pin
    pub interrupt_pin: u8,
}

impl PciDeviceInfo {
    /// Get the device name based on class code
    pub fn device_type_name(&self) -> &'static str {
        match self.class_code {
            0x00 => "Unclassified",
            0x01 => "Mass Storage Controller",
            0x02 => "Network Controller",
            0x03 => "Display Controller",
            0x04 => "Multimedia Controller",
            0x05 => "Memory Controller",
            0x06 => "Bridge Device",
            0x07 => "Communication Controller",
            0x08 => "System Peripheral",
            0x09 => "Input Device",
            0x0A => "Docking Station",
            0x0B => "Processor",
            0x0C => "Serial Bus Controller",
            0x0D => "Wireless Controller",
            0x0E => "Intelligent I/O Controller",
            0x0F => "Satellite Communication Controller",
            0x10 => "Encryption/Decryption Controller",
            0x11 => "Data Acquisition Controller",
            _ => "Unknown",
        }
    }

    /// Check if this is a network controller
    pub fn is_network_controller(&self) -> bool {
        self.class_code == 0x02
    }

    /// Check if this is a PCI-to-PCI bridge
    pub fn is_pci_bridge(&self) -> bool {
        self.class_code == 0x06 && self.subclass == 0x04
    }

    /// Get the physical address of a BAR
    pub fn bar_address(&self, bar_index: usize) -> Option<PhysAddr> {
        if bar_index >= 6 {
            return None;
        }

        let bar = self.bars[bar_index];
        if bar == 0 {
            return None;
        }

        // Check if it's an I/O space BAR (bit 0 set)
        if bar & 0x1 != 0 {
            // I/O space - not commonly used on ARM64
            return None;
        }

        // Memory space BAR
        // Bit 1-2: Type (00 = 32-bit, 10 = 64-bit)
        let bar_type = (bar >> 1) & 0x3;

        match bar_type {
            0 => {
                // 32-bit address
                let addr = bar & !0xF; // Clear lower 4 bits
                Some(PhysAddr::from(addr as usize))
            }
            2 => {
                // 64-bit address
                if bar_index >= 5 {
                    return None; // Need next BAR for upper 32 bits
                }
                let low = (bar & !0xF) as u64;
                let high = self.bars[bar_index + 1] as u64;
                let addr = (high << 32) | low;
                Some(PhysAddr::from(addr as usize))
            }
            _ => None,
        }
    }
}

/// PCI bus configuration
pub struct PciConfig {
    /// ECAM base physical address
    ecam_base: PhysAddr,
    /// Bus range start
    bus_start: u8,
    /// Bus range end
    bus_end: u8,
}

impl PciConfig {
    /// Create a new PCI configuration
    ///
    /// # Arguments
    /// * `ecam_base` - Physical address of ECAM configuration space
    /// * `bus_start` - Starting bus number
    /// * `bus_end` - Ending bus number
    pub fn new(ecam_base: PhysAddr, bus_start: u8, bus_end: u8) -> Self {
        Self {
            ecam_base,
            bus_start,
            bus_end,
        }
    }

    /// Calculate configuration space address for a device
    ///
    /// ECAM address format: BASE + (bus << 20 | device << 15 | function << 12)
    fn config_address(&self, bus: u8, device: u8, function: u8) -> usize {
        let offset =
            ((bus as usize) << 20) | ((device as usize) << 15) | ((function as usize) << 12);
        self.ecam_base.as_usize() + offset / 4
    }

    /// Read PCI configuration header
    fn read_config_header(&self, bus: u8, device: u8, function: u8) -> Option<PciConfigHeader> {
        let phys_addr = self.config_address(bus, device, function);
        let virt_addr = phys_to_virt(PhysAddr::from(phys_addr));

        unsafe {
            let header_ptr = virt_addr.as_ptr() as *const PciConfigHeader;
            let header = header_ptr.read_volatile();

            // Check for valid vendor ID (0xFFFF means no device)
            if header.vendor_id == 0xFFFF || header.vendor_id == 0x0000 {
                return None;
            }

            Some(header)
        }
    }

    /// Read PCI bridge header (Header Type 1)
    fn read_bridge_header(&self, bus: u8, device: u8, function: u8) -> Option<PciBridgeHeader> {
        let phys_addr = self.config_address(bus, device, function);
        info!("Reading PCI bridge header at phys addr {:#x}", phys_addr);
        let virt_addr = phys_to_virt(PhysAddr::from(phys_addr));

        unsafe {
            let header_ptr = virt_addr.as_ptr() as *const PciBridgeHeader;
            let header = header_ptr.read_volatile();

            // Check for valid vendor ID
            if header.vendor_id == 0xFFFF || header.vendor_id == 0x0000 {
                return None;
            }

            // Verify this is actually a bridge (header type 1)
            let header_type = header.header_type & 0x7F;
            if header_type != 0x01 {
                return None;
            }

            Some(header)
        }
    }

    /// Probe a single PCI device
    fn probe_device(&self, bus: u8, device: u8, function: u8) -> Option<PciDeviceInfo> {
        let header = self.read_config_header(bus, device, function)?;

        Some(PciDeviceInfo {
            bus,
            device,
            function,
            vendor_id: header.vendor_id,
            device_id: header.device_id,
            class_code: header.class_code,
            subclass: header.subclass,
            prog_if: header.prog_if,
            revision_id: header.revision_id,
            bars: header.bar,
            interrupt_line: header.interrupt_line,
            interrupt_pin: header.interrupt_pin,
        })
    }

    /// Probe all PCI devices on the bus
    pub fn probe_all(&self) -> alloc::vec::Vec<PciDeviceInfo> {
        let mut devices = alloc::vec::Vec::new();

        for bus in self.bus_start..=self.bus_end {
            self.probe_bus(bus, &mut devices);
        }

        devices
    }

    /// Recursively probe a single bus and its bridges
    fn probe_bus(&self, bus: u8, devices: &mut alloc::vec::Vec<PciDeviceInfo>) {
        for device in 0..32 {
            // Check function 0 first
            if let Some(dev_info) = self.probe_device(bus, device, 0) {
                devices.push(dev_info.clone());

                // If this is a PCI bridge, scan the secondary bus
                if dev_info.is_pci_bridge() {
                    if let Some(bridge) = self.read_bridge_header(bus, device, 0) {
                        // Only scan if the secondary bus is within our configured range
                        if bridge.secondary_bus >= self.bus_start
                            && bridge.secondary_bus <= self.bus_end
                        {
                            info!(
                                "  Found PCI bridge at {:02x}:{:02x}.0 -> secondary bus {:02x}",
                                bus, device, bridge.secondary_bus
                            );
                            // Recursively scan the secondary bus
                            self.probe_bus(bridge.secondary_bus, devices);
                        } else {
                            warn!(
                                "  Skipping PCI bridge at {:02x}:{:02x}.0 (secondary bus {:02x} out of range {:02x}-{:02x})",
                                bus, device, bridge.secondary_bus, self.bus_start, self.bus_end
                            );
                        }
                    }
                }

                // If this is a multi-function device, check other functions
                let header = self.read_config_header(bus, device, 0).unwrap();
                if header.header_type & 0x80 != 0 {
                    // Multi-function device
                    for function in 1..8 {
                        if let Some(func_info) = self.probe_device(bus, device, function) {
                            devices.push(func_info.clone());

                            // Check if this function is also a bridge
                            if func_info.is_pci_bridge() {
                                if let Some(bridge) = self.read_bridge_header(bus, device, function)
                                {
                                    // Only scan if the secondary bus is within our configured range
                                    if bridge.secondary_bus >= self.bus_start
                                        && bridge.secondary_bus <= self.bus_end
                                    {
                                        info!(
                                            "  Found PCI bridge at {:02x}:{:02x}.{} -> secondary bus {:02x}",
                                            bus, device, function, bridge.secondary_bus
                                        );
                                        self.probe_bus(bridge.secondary_bus, devices);
                                    } else {
                                        warn!(
                                            "  Skipping PCI bridge at {:02x}:{:02x}.{} (secondary bus {:02x} out of range {:02x}-{:02x})",
                                            bus,
                                            device,
                                            function,
                                            bridge.secondary_bus,
                                            self.bus_start,
                                            self.bus_end
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Find devices by vendor and device ID
    pub fn find_device(&self, vendor_id: u16, device_id: u16) -> Option<PciDeviceInfo> {
        for bus in self.bus_start..=self.bus_end {
            for device in 0..32 {
                for function in 0..8 {
                    if let Some(dev_info) = self.probe_device(bus, device, function) {
                        if dev_info.vendor_id == vendor_id && dev_info.device_id == device_id {
                            return Some(dev_info);
                        }
                    }
                }
            }
        }
        None
    }

    /// Find devices by class code
    pub fn find_devices_by_class(&self, class_code: u8) -> alloc::vec::Vec<PciDeviceInfo> {
        let mut devices = alloc::vec::Vec::new();

        for bus in self.bus_start..=self.bus_end {
            for device in 0..32 {
                for function in 0..8 {
                    if let Some(dev_info) = self.probe_device(bus, device, function) {
                        if dev_info.class_code == class_code {
                            devices.push(dev_info);
                        }
                    }
                }
            }
        }

        devices
    }
}

// ============================================================================
// DesignWare PCIe ATU (Address Translation Unit) Support
// ============================================================================

/// DBI (DesignWare Bus Interface) register base address for RK3588
#[allow(dead_code)]
const DBI_BASE: u64 = 0xa40c00000;

/// iATU Unroll mode register offsets
#[allow(dead_code)]
const PCIE_ATU_UNR_REGION_CTRL1: usize = 0x00;
#[allow(dead_code)]
const PCIE_ATU_UNR_REGION_CTRL2: usize = 0x04;
#[allow(dead_code)]
const PCIE_ATU_UNR_LOWER_BASE: usize = 0x08;
#[allow(dead_code)]
const PCIE_ATU_UNR_UPPER_BASE: usize = 0x0C;
#[allow(dead_code)]
const PCIE_ATU_UNR_LOWER_LIMIT: usize = 0x10;
#[allow(dead_code)]
const PCIE_ATU_UNR_UPPER_LIMIT: usize = 0x14;
#[allow(dead_code)]
const PCIE_ATU_UNR_LOWER_TARGET: usize = 0x14;
#[allow(dead_code)]
const PCIE_ATU_UNR_UPPER_TARGET: usize = 0x18;

/// iATU Unroll base address offset (DBI + 0x300000)
#[allow(dead_code)]
const DEFAULT_DBI_ATU_OFFSET: u64 = 0x3 << 20; // 0x300000

/// iATU Enable bit
#[allow(dead_code)]
const PCIE_ATU_ENABLE: u32 = 1 << 31;

/// iATU Type: Config Type 0
#[allow(dead_code)]
const PCIE_ATU_TYPE_CFG0: u32 = 0x4;
/// iATU Type: Config Type 1
#[allow(dead_code)]
const PCIE_ATU_TYPE_CFG1: u32 = 0x5;
/// iATU Type: I/O
#[allow(dead_code)]
const PCIE_ATU_TYPE_IO: u32 = 0x2;
/// iATU Type: Memory
#[allow(dead_code)]
const PCIE_ATU_TYPE_MEM: u32 = 0x0;

/// iATU Region Index for configuration access
#[allow(dead_code)]
const PCIE_ATU_REGION_INDEX1: u32 = 1;

/// Maximum retries for iATU enable
const LINK_WAIT_MAX_IATU_RETRIES: usize = 5;

/// Get outbound iATU region register offset
/// Each region is 512 bytes (region << 9)
#[inline]
const fn get_atu_outb_unr_reg_offset(region: u32) -> usize {
    (region as usize) << 9
}

/// DesignWare PCIe controller ATU operations
pub struct DwPcieAtu {
    dbi_base_virt: usize,
    atu_base_virt: usize,
}

impl DwPcieAtu {
    /// Create new ATU accessor
    pub fn new() -> Self {
        let dbi_base_virt = phys_to_virt(PhysAddr::from(DBI_BASE as usize)).as_usize();
        let atu_base_virt = dbi_base_virt + (DEFAULT_DBI_ATU_OFFSET as usize);

        info!("DesignWare PCIe ATU Initialization:");
        info!("  DBI Base:  phys={:#010x}, virt={:#018x}", DBI_BASE, dbi_base_virt);
        info!("  ATU Base:  virt={:#018x}", atu_base_virt);

        Self {
            dbi_base_virt,
            atu_base_virt,
        }
    }

    /// Write to outbound ATU register (Unroll mode)
    #[inline]
    fn writel_ob_unroll(&self, index: u32, reg: usize, val: u32) {
        let offset = get_atu_outb_unr_reg_offset(index);
        let addr = (self.atu_base_virt + offset + reg) as *mut u32;
        unsafe {
            core::ptr::write_volatile(addr, val);
        }
    }

    /// Read from outbound ATU register (Unroll mode)
    #[inline]
    fn readl_ob_unroll(&self, index: u32, reg: usize) -> u32 {
        let offset = get_atu_outb_unr_reg_offset(index);
        let addr = (self.atu_base_virt + offset + reg) as *const u32;
        unsafe { core::ptr::read_volatile(addr) }
    }

    /// Program outbound ATU (Unroll mode)
    ///
    /// # Parameters
    /// * `index` - ATU region index
    /// * `atu_type` - ATU access type (CFG0, CFG1, MEM, IO)
    /// * `cpu_addr` - Physical address for the translation entry (source)
    /// * `pci_addr` - PCIe bus address for the translation entry (target)
    /// * `size` - Size of the translation entry
    ///
    /// # Returns
    /// * `Ok(())` on success
    /// * `Err(&str)` on failure
    pub fn prog_outbound_atu(
        &self,
        index: u32,
        atu_type: u32,
        cpu_addr: u64,
        pci_addr: u64,
        size: u32,
    ) -> Result<(), &'static str> {
        info!(
            "ATU[{}]: type={:#x}, cpu={:#016x}, pci={:#016x}, size={:#x}",
            index, atu_type, cpu_addr, pci_addr, size
        );

        // Configure lower and upper base (source CPU address)
        let lower_base = (cpu_addr & 0xFFFFFFFF) as u32;
        let upper_base = (cpu_addr >> 32) as u32;

        self.writel_ob_unroll(index, PCIE_ATU_UNR_LOWER_BASE, lower_base);
        self.writel_ob_unroll(index, PCIE_ATU_UNR_UPPER_BASE, upper_base);

        // Configure limit (end of source address range)
        let limit_addr = cpu_addr + (size as u64) - 1;
        let lower_limit = (limit_addr & 0xFFFFFFFF) as u32;
        let upper_limit = (limit_addr >> 32) as u32;

        self.writel_ob_unroll(index, PCIE_ATU_UNR_LOWER_LIMIT, lower_limit);
        self.writel_ob_unroll(index, PCIE_ATU_UNR_UPPER_LIMIT, upper_limit);

        // Configure target address (PCIe bus address)
        let lower_target = (pci_addr & 0xFFFFFFFF) as u32;
        let upper_target = (pci_addr >> 32) as u32;

        self.writel_ob_unroll(index, PCIE_ATU_UNR_LOWER_TARGET, lower_target);
        self.writel_ob_unroll(index, PCIE_ATU_UNR_UPPER_TARGET, upper_target);

        // Configure region control (transaction type)
        self.writel_ob_unroll(index, PCIE_ATU_UNR_REGION_CTRL1, atu_type);

        // Enable ATU region
        self.writel_ob_unroll(index, PCIE_ATU_UNR_REGION_CTRL2, PCIE_ATU_ENABLE);

        // Wait for ATU enable to take effect
        for retry in 0..LINK_WAIT_MAX_IATU_RETRIES {
            let val = self.readl_ob_unroll(index, PCIE_ATU_UNR_REGION_CTRL2);
            if (val & PCIE_ATU_ENABLE) != 0 {
                if retry > 0 {
                    debug!("ATU[{}] enabled after {} retries", index, retry);
                }
                return Ok(());
            }
            // Small delay
            for _ in 0..100_000 {
                core::hint::spin_loop();
            }
        }

        error!("ATU[{}] enable timeout!", index);
        Err("Outbound iATU is not being enabled")
    }

    /// Dump ATU region configuration for debugging
    pub fn dump_atu_config(&self, index: u32) {
        let lower_base = self.readl_ob_unroll(index, PCIE_ATU_UNR_LOWER_BASE);
        let upper_base = self.readl_ob_unroll(index, PCIE_ATU_UNR_UPPER_BASE);
        let lower_limit = self.readl_ob_unroll(index, PCIE_ATU_UNR_LOWER_LIMIT);
        let upper_limit = self.readl_ob_unroll(index, PCIE_ATU_UNR_UPPER_LIMIT);
        let lower_target = self.readl_ob_unroll(index, PCIE_ATU_UNR_LOWER_TARGET);
        let upper_target = self.readl_ob_unroll(index, PCIE_ATU_UNR_UPPER_TARGET);
        let ctrl1 = self.readl_ob_unroll(index, PCIE_ATU_UNR_REGION_CTRL1);
        let ctrl2 = self.readl_ob_unroll(index, PCIE_ATU_UNR_REGION_CTRL2);

        info!("ATU Region {} Configuration:", index);
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
        info!("  Source (CPU):     {:#010x}_{:08x}", upper_base, lower_base);
        info!("  Limit:            {:#010x}_{:08x}", upper_limit, lower_limit);
        info!("  Target (PCIe):    {:#010x}_{:08x}", upper_target, lower_target);
    }
}

// ============================================================================
// Standard ECAM Configuration Space Access
// ============================================================================

/// Default PCI configuration for RK3588 PCIe controller
/// Based on device tree: pcie@fe180000
pub fn default_pci_config() -> PciConfig {
    // From device tree:
    // reg = <0x00 0xfe180000 0x00 0x10000 0x0a 0x40c00000 0x00 0x400000>;
    // bus-range = <0x30 0x3f>;
    // Configuration space is at physical address 0x0a_40c00000 (42GB)
    const ECAM_BASE: u64 = 0x0a_40c00000; // Configuration space base
    const BUS_START: u8 = 0x0;
    const BUS_END: u8 = 0x3f;

    PciConfig::new(PhysAddr::from(ECAM_BASE as usize), BUS_START, BUS_END)
}

/// Probe and print all PCI devices
pub fn probe_pci_devices() {
    info!("Starting PCI device enumeration...");

    let pci_config = default_pci_config();
    let devices = pci_config.probe_all();

    if devices.is_empty() {
        info!("No PCI devices found.");
        return;
    }

    info!("Found {} PCI device(s):", devices.len());
    for dev in &devices {
        info!(
            "  [{:02x}:{:02x}.{}] Vendor: {:#06x}, Device: {:#06x}, Class: {} ({:#04x}:{:#04x})",
            dev.bus,
            dev.device,
            dev.function,
            dev.vendor_id,
            dev.device_id,
            dev.device_type_name(),
            dev.class_code,
            dev.subclass,
        );

        // Print BARs
        for (i, &bar) in dev.bars.iter().enumerate() {
            if bar != 0 {
                if let Some(addr) = dev.bar_address(i) {
                    info!("    BAR{}: {:#x}", i, addr);
                }
            }
        }

        if dev.interrupt_pin != 0 {
            info!(
                "    IRQ: line={}, pin={}",
                dev.interrupt_line, dev.interrupt_pin
            );
        }
    }
}

/// Find and return network controllers
pub fn find_network_controllers() -> alloc::vec::Vec<PciDeviceInfo> {
    let pci_config = default_pci_config();
    pci_config.find_devices_by_class(0x02) // Network controller class
}

/// Test DesignWare PCIe ATU functionality
/// 
/// This function demonstrates how to use the ATU to configure outbound
/// address translation for PCIe configuration space access.
#[allow(dead_code)]
pub fn test_dw_pcie_atu() {
    info!("=== Testing DesignWare PCIe ATU ===");

    let atu = DwPcieAtu::new();

    // Example: Configure ATU for configuration space access
    // Region 1, Type CFG0, CPU address -> PCIe bus address
    let cpu_addr = 0xf300_0000u64; // Configuration window
    let pci_addr = 0x0000_0000u64; // Bus 0, Device 0, Function 0
    let size = 0x10_0000u32; // 1MB window

    info!("Configuring ATU region 1 for configuration access");
    match atu.prog_outbound_atu(PCIE_ATU_REGION_INDEX1, PCIE_ATU_TYPE_CFG0, cpu_addr, pci_addr, size) {
        Ok(_) => {
            info!("ATU configuration successful");
            atu.dump_atu_config(PCIE_ATU_REGION_INDEX1);

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

