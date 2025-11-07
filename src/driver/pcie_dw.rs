//! DesignWare PCIe Controller Driver
//!
//! This module implements PCIe configuration space access for Synopsys DesignWare
//! PCIe controllers using the iATU (internal Address Translation Unit).
//!
//! The iATU is required because DW PCIe controllers don't support standard ECAM.
//! Instead, they use a small configuration window that must be dynamically remapped
//! to different PCIe devices using the iATU hardware.

extern crate alloc;

use crate::arch::mem::phys_to_virt;
use core::ptr;
use memory_addr::{PhysAddr, VirtAddr};

/// PCIe DBI (DesignWare Bus Interface) register base address
/// Obtained from device tree, value for RK3588
const DBI_BASE_PHYS: usize = 0xfe180000;

/// Configuration space window physical base address
/// This is the iATU input window address
const CFG_WINDOW_BASE_PHYS: usize = 0xf3000000;

/// Configuration space window size
const CFG_WINDOW_SIZE: usize = 0x100000; // 1MB

// ============================================================================
// iATU register offsets (Viewport mode)
// ============================================================================

const PCIE_ATU_VIEWPORT: usize = 0x900;
const PCIE_ATU_CR1: usize = 0x904;
const PCIE_ATU_CR2: usize = 0x908;
const PCIE_ATU_LOWER_BASE: usize = 0x90C;
const PCIE_ATU_UPPER_BASE: usize = 0x910;
const PCIE_ATU_LIMIT: usize = 0x914;
const PCIE_ATU_LOWER_TARGET: usize = 0x918;
const PCIE_ATU_UPPER_TARGET: usize = 0x91C;

// ============================================================================
// iATU Unroll mode register offsets
// ============================================================================

const PCIE_ATU_UNR_REGION_CTRL1: usize = 0x00;
const PCIE_ATU_UNR_REGION_CTRL2: usize = 0x04;
const PCIE_ATU_UNR_LOWER_BASE: usize = 0x08;
const PCIE_ATU_UNR_UPPER_BASE: usize = 0x0C;
const PCIE_ATU_UNR_LOWER_LIMIT: usize = 0x10;
const PCIE_ATU_UNR_LOWER_TARGET: usize = 0x14;
const PCIE_ATU_UNR_UPPER_TARGET: usize = 0x18;

/// iATU Unroll base address offset
const DEFAULT_DBI_ATU_OFFSET: usize = 0x3 << 20; // 0x300000

/// Get outbound iATU region register offset
#[inline]
const fn get_atu_outb_unr_reg_offset(region: u32) -> usize {
    (region as usize) << 9 // region * 512
}

// ============================================================================
// iATU constants
// ============================================================================

#[allow(dead_code)]
/// iATU Type: Memory
const PCIE_ATU_TYPE_MEM: u32 = 0x0;
#[allow(dead_code)]
/// iATU Type: I/O
const PCIE_ATU_TYPE_IO: u32 = 0x2;
/// iATU Type: Config Type 0 (access devices on the same bus)
const PCIE_ATU_TYPE_CFG0: u32 = 0x4;
/// iATU Type: Config Type 1 (access devices on downstream buses)
const PCIE_ATU_TYPE_CFG1: u32 = 0x5;

/// iATU Enable bit
const PCIE_ATU_ENABLE: u32 = 1 << 31;

/// iATU region direction: Outbound
const PCIE_ATU_REGION_OUTBOUND: u32 = 0;

/// Maximum retries for iATU enable
const IATU_ENABLE_MAX_RETRIES: usize = 5;

// ============================================================================
// PCIe configuration space access structure
// ============================================================================

/// DesignWare PCIe configuration space accessor
pub struct DwPcieConfigAccess {
    dbi_base_virt: VirtAddr,
    cfg_window_virt: VirtAddr,
    atu_base_virt: VirtAddr, // iATU Unroll mode base address
    use_unroll: bool,        // Whether to use Unroll mode
}

impl DwPcieConfigAccess {
    /// Create new configuration space accessor
    pub fn new() -> Self {
        let dbi_base_virt = phys_to_virt(PhysAddr::from(DBI_BASE_PHYS));
        let cfg_window_virt = phys_to_virt(PhysAddr::from(CFG_WINDOW_BASE_PHYS));

        // RK3588 uses Unroll mode, iATU registers are at DBI + 0x300000
        let use_unroll = true;
        let atu_base_virt = if use_unroll {
            VirtAddr::from(dbi_base_virt.as_usize() + DEFAULT_DBI_ATU_OFFSET)
        } else {
            dbi_base_virt
        };

        info!("DW PCIe Controller Initialization:");
        info!(
            "  DBI Base:      phys={:#010x}, virt={:#018x}",
            DBI_BASE_PHYS,
            dbi_base_virt.as_usize()
        );
        info!(
            "  Config Window: phys={:#010x}, virt={:#018x}",
            CFG_WINDOW_BASE_PHYS,
            cfg_window_virt.as_usize()
        );
        info!(
            "  Window Size:   {:#x} ({} KB)",
            CFG_WINDOW_SIZE,
            CFG_WINDOW_SIZE / 1024
        );
        info!(
            "  iATU Mode:     {}",
            if use_unroll { "Unroll" } else { "Viewport" }
        );
        if use_unroll {
            info!("  iATU Base:     virt={:#018x}", atu_base_virt.as_usize());
        }

        Self {
            dbi_base_virt,
            cfg_window_virt,
            atu_base_virt,
            use_unroll,
        }
    }

    /// Write DBI register
    #[inline]
    fn write_dbi(&self, offset: usize, value: u32) {
        unsafe {
            let addr = (self.dbi_base_virt.as_usize() + offset) as *mut u32;
            ptr::write_volatile(addr, value);
        }
    }

    /// Read DBI register
    #[inline]
    fn read_dbi(&self, offset: usize) -> u32 {
        unsafe {
            let addr = (self.dbi_base_virt.as_usize() + offset) as *const u32;
            ptr::read_volatile(addr)
        }
    }

    /// Configure outbound iATU (Unroll mode)
    fn program_outbound_atu_unroll(
        &self,
        index: u32,
        atu_type: u32,
        cpu_addr: u64,
        pci_addr: u64,
        size: u64,
    ) -> Result<(), &'static str> {
        let region_offset = get_atu_outb_unr_reg_offset(index);

        // 1. Configure source address range (CPU side)
        let lower_base = cpu_addr as u32;
        let upper_base = (cpu_addr >> 32) as u32;
        let limit = ((cpu_addr + size - 1) & 0xFFFFFFFF) as u32;

        unsafe {
            let base = self.atu_base_virt.as_usize() + region_offset;
            ptr::write_volatile((base + PCIE_ATU_UNR_LOWER_BASE) as *mut u32, lower_base);
            ptr::write_volatile((base + PCIE_ATU_UNR_UPPER_BASE) as *mut u32, upper_base);
            ptr::write_volatile((base + PCIE_ATU_UNR_LOWER_LIMIT) as *mut u32, limit);

            // 2. Configure target address (PCIe bus address)
            let lower_target = pci_addr as u32;
            let upper_target = (pci_addr >> 32) as u32;

            ptr::write_volatile((base + PCIE_ATU_UNR_LOWER_TARGET) as *mut u32, lower_target);
            ptr::write_volatile((base + PCIE_ATU_UNR_UPPER_TARGET) as *mut u32, upper_target);

            // 3. Configure transaction type
            ptr::write_volatile((base + PCIE_ATU_UNR_REGION_CTRL1) as *mut u32, atu_type);

            // 4. Enable iATU
            ptr::write_volatile(
                (base + PCIE_ATU_UNR_REGION_CTRL2) as *mut u32,
                PCIE_ATU_ENABLE,
            );

            // 5. Wait for iATU enable to take effect
            for retry in 0..IATU_ENABLE_MAX_RETRIES {
                let cr2 = ptr::read_volatile((base + PCIE_ATU_UNR_REGION_CTRL2) as *const u32);
                if cr2 & PCIE_ATU_ENABLE != 0 {
                    if retry > 0 {
                        debug!(
                            "iATU region {} enabled after {} retries (Unroll)",
                            index, retry
                        );
                    }
                    return Ok(());
                }
                // Delay ~9ms
                for _ in 0..900_000 {
                    core::hint::spin_loop();
                }
            }
        }

        error!("iATU region {} enable timeout (Unroll mode)!", index);
        Err("iATU enable timeout (Unroll mode)")
    }

    /// Configure outbound iATU (Viewport mode)
    fn program_outbound_atu_viewport(
        &self,
        index: u32,
        atu_type: u32,
        cpu_addr: u64,
        pci_addr: u64,
        size: u64,
    ) -> Result<(), &'static str> {
        // 1. Select iATU region (viewport)
        self.write_dbi(PCIE_ATU_VIEWPORT, PCIE_ATU_REGION_OUTBOUND | (index & 0xF));

        // 2. Configure source address range (CPU side)
        let lower_base = cpu_addr as u32;
        let upper_base = (cpu_addr >> 32) as u32;
        let limit = ((cpu_addr + size - 1) & 0xFFFFFFFF) as u32;

        self.write_dbi(PCIE_ATU_LOWER_BASE, lower_base);
        self.write_dbi(PCIE_ATU_UPPER_BASE, upper_base);
        self.write_dbi(PCIE_ATU_LIMIT, limit);

        // 3. Configure target address (PCIe bus address)
        let lower_target = pci_addr as u32;
        let upper_target = (pci_addr >> 32) as u32;

        self.write_dbi(PCIE_ATU_LOWER_TARGET, lower_target);
        self.write_dbi(PCIE_ATU_UPPER_TARGET, upper_target);

        // 4. Configure transaction type
        self.write_dbi(PCIE_ATU_CR1, atu_type);

        // 5. Enable iATU
        self.write_dbi(PCIE_ATU_CR2, PCIE_ATU_ENABLE);

        // 6. Wait for iATU enable to take effect
        for retry in 0..IATU_ENABLE_MAX_RETRIES {
            let cr2 = self.read_dbi(PCIE_ATU_CR2);
            if cr2 & PCIE_ATU_ENABLE != 0 {
                if retry > 0 {
                    debug!(
                        "iATU region {} enabled after {} retries (Viewport)",
                        index, retry
                    );
                }
                return Ok(());
            }
            // Delay ~9ms
            for _ in 0..900_000 {
                core::hint::spin_loop();
            }
        }

        error!("iATU region {} enable timeout (Viewport mode)!", index);
        Err("iATU enable timeout (Viewport mode)")
    }

    /// Configure outbound iATU
    ///
    /// # Parameters
    /// * `index` - iATU region index (usually 0)
    /// * `atu_type` - iATU type (CFG0/CFG1/MEM/IO)
    /// * `cpu_addr` - CPU side physical address (window start address)
    /// * `pci_addr` - PCIe bus address (target BDF encoding)
    /// * `size` - Window size
    fn program_outbound_atu(
        &self,
        index: u32,
        atu_type: u32,
        cpu_addr: u64,
        pci_addr: u64,
        size: u64,
    ) -> Result<(), &'static str> {
        if self.use_unroll {
            self.program_outbound_atu_unroll(index, atu_type, cpu_addr, pci_addr, size)
        } else {
            self.program_outbound_atu_viewport(index, atu_type, cpu_addr, pci_addr, size)
        }
    }

    /// Read configuration space DWORD (32-bit)
    ///
    /// # Parameters
    /// * `bus` - Bus number
    /// * `dev` - Device number
    /// * `func` - Function number
    /// * `reg` - Register offset (must be 4-byte aligned)
    pub fn read_config_dword(
        &self,
        bus: u8,
        dev: u8,
        func: u8,
        reg: u16,
    ) -> Result<u32, &'static str> {
        // Check alignment
        if reg & 0x3 != 0 {
            return Err("Register offset must be 4-byte aligned");
        }

        // 1. Encode busdev (PCIe bus address)
        // Format: [31:24]=bus, [23:19]=dev, [18:16]=func, [15:0]=0
        let busdev: u64 = ((bus as u64) << 24) | ((dev as u64) << 19) | ((func as u64) << 16);

        // 2. Determine configuration space type
        let cfg_type = if bus == 0 || bus == 1 {
            PCIE_ATU_TYPE_CFG0 // Type 0: directly connected devices
        } else {
            PCIE_ATU_TYPE_CFG1 // Type 1: devices accessed through bridges
        };

        // 3. Critical step: configure iATU
        self.program_outbound_atu(
            0, // Use region 0
            cfg_type,
            CFG_WINDOW_BASE_PHYS as u64,
            busdev,
            CFG_WINDOW_SIZE as u64,
        )?;

        // 4. Read data through configuration window
        // Note: accessing virtual address here
        let addr = (self.cfg_window_virt.as_usize() + reg as usize) as *const u32;
        let value = unsafe { ptr::read_volatile(addr) };

        Ok(value)
    }

    /// Write configuration space DWORD (32-bit)
    #[allow(dead_code)]
    pub fn write_config_dword(
        &self,
        bus: u8,
        dev: u8,
        func: u8,
        reg: u16,
        value: u32,
    ) -> Result<(), &'static str> {
        if reg & 0x3 != 0 {
            return Err("Register offset must be 4-byte aligned");
        }

        let busdev: u64 = ((bus as u64) << 24) | ((dev as u64) << 19) | ((func as u64) << 16);

        let cfg_type = if bus == 0 || bus == 1 {
            PCIE_ATU_TYPE_CFG0
        } else {
            PCIE_ATU_TYPE_CFG1
        };

        self.program_outbound_atu(
            0,
            cfg_type,
            CFG_WINDOW_BASE_PHYS as u64,
            busdev,
            CFG_WINDOW_SIZE as u64,
        )?;

        let addr = (self.cfg_window_virt.as_usize() + reg as usize) as *mut u32;
        unsafe { ptr::write_volatile(addr, value) };

        Ok(())
    }

    /// Read Vendor ID and Device ID
    pub fn read_vendor_device_id(
        &self,
        bus: u8,
        dev: u8,
        func: u8,
    ) -> Result<(u16, u16), &'static str> {
        let val = self.read_config_dword(bus, dev, func, 0x00)?;

        // Check if valid
        if val == 0xFFFFFFFF || val == 0 {
            return Err("No device present");
        }

        let vendor_id = (val & 0xFFFF) as u16;
        let device_id = ((val >> 16) & 0xFFFF) as u16;

        Ok((vendor_id, device_id))
    }

    /// Read device class information
    pub fn read_class_info(
        &self,
        bus: u8,
        dev: u8,
        func: u8,
    ) -> Result<(u8, u8, u8), &'static str> {
        let val = self.read_config_dword(bus, dev, func, 0x08)?;

        let _revision_id = (val & 0xFF) as u8;
        let prog_if = ((val >> 8) & 0xFF) as u8;
        let subclass = ((val >> 16) & 0xFF) as u8;
        let class_code = ((val >> 24) & 0xFF) as u8;

        Ok((class_code, subclass, prog_if))
    }

    /// Read Header Type
    pub fn read_header_type(&self, bus: u8, dev: u8, func: u8) -> Result<u8, &'static str> {
        let val = self.read_config_dword(bus, dev, func, 0x0C)?;
        let header_type = ((val >> 16) & 0xFF) as u8;
        Ok(header_type)
    }

    /// Read BAR (Base Address Register)
    pub fn read_bar(&self, bus: u8, dev: u8, func: u8, bar_index: u8) -> Result<u32, &'static str> {
        if bar_index >= 6 {
            return Err("BAR index must be 0-5");
        }
        let reg = 0x10 + (bar_index as u16) * 4;
        self.read_config_dword(bus, dev, func, reg)
    }

    /// Print iATU configuration status (for debugging)
    pub fn dump_iatu_config(&self, region: u32) {
        self.write_dbi(PCIE_ATU_VIEWPORT, PCIE_ATU_REGION_OUTBOUND | (region & 0xF));

        let cr1 = self.read_dbi(PCIE_ATU_CR1);
        let cr2 = self.read_dbi(PCIE_ATU_CR2);
        let lower_base = self.read_dbi(PCIE_ATU_LOWER_BASE);
        let upper_base = self.read_dbi(PCIE_ATU_UPPER_BASE);
        let limit = self.read_dbi(PCIE_ATU_LIMIT);
        let lower_target = self.read_dbi(PCIE_ATU_LOWER_TARGET);
        let upper_target = self.read_dbi(PCIE_ATU_UPPER_TARGET);

        info!("iATU Region {} Configuration:", region);
        info!("  CR1 (Type):       {:#010x}", cr1);
        info!(
            "  CR2 (Enable):     {:#010x} {}",
            cr2,
            if cr2 & PCIE_ATU_ENABLE != 0 {
                "[ENABLED]"
            } else {
                "[DISABLED]"
            }
        );
        info!(
            "  Source (CPU):     {:#010x}_{:08x}",
            upper_base, lower_base
        );
        info!("  Limit:            {:#010x}", limit);
        info!(
            "  Target (PCIe):    {:#010x}_{:08x}",
            upper_target, lower_target
        );
    }
}

// ============================================================================
// Device scanning functionality
// ============================================================================

/// PCIe device information
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PcieDeviceInfo {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub bars: [u32; 6],
}

impl PcieDeviceInfo {
    /// Get device type name
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

    /// Is this a network controller
    pub fn is_network_controller(&self) -> bool {
        self.class_code == 0x02
    }
}

/// Scan all devices on PCIe bus
pub fn scan_pcie_devices() -> alloc::vec::Vec<PcieDeviceInfo> {
    let pcie = DwPcieConfigAccess::new();
    let mut devices = alloc::vec::Vec::new();

    info!("=== Starting PCIe Bus Scan ===");

    // Scan bus 0-255
    for bus in 0..=255u8 {
        let mut device_found_on_bus = false;

        // Maximum 32 devices per bus
        for dev in 0..32u8 {
            // First check function 0
            match pcie.read_vendor_device_id(bus, dev, 0) {
                Ok((vendor_id, device_id)) => {
                    device_found_on_bus = true;

                    // Read class information
                    let (class_code, subclass, prog_if) = pcie
                        .read_class_info(bus, dev, 0)
                        .unwrap_or((0xFF, 0xFF, 0xFF));

                    // Read BARs
                    let mut bars = [0u32; 6];
                    for i in 0..6 {
                        bars[i] = pcie.read_bar(bus, dev, 0, i as u8).unwrap_or(0);
                    }

                    let dev_info = PcieDeviceInfo {
                        bus,
                        dev,
                        func: 0,
                        vendor_id,
                        device_id,
                        class_code,
                        subclass,
                        prog_if,
                        bars,
                    };

                    info!(
                        "Found Device: Bus {:02x}, Dev {:02x}, Func {:x} - {:04x}:{:04x} ({})",
                        bus,
                        dev,
                        0,
                        vendor_id,
                        device_id,
                        dev_info.device_type_name()
                    );

                    devices.push(dev_info);

                    // Check if it's a multi-function device
                    let header_type = pcie.read_header_type(bus, dev, 0).unwrap_or(0);
                    let is_multi_function = (header_type & 0x80) != 0;

                    if is_multi_function {
                        // Scan other functions
                        for func in 1..8u8 {
                            if let Ok((vendor_id, device_id)) =
                                pcie.read_vendor_device_id(bus, dev, func)
                            {
                                let (class_code, subclass, prog_if) = pcie
                                    .read_class_info(bus, dev, func)
                                    .unwrap_or((0xFF, 0xFF, 0xFF));

                                let mut bars = [0u32; 6];
                                for i in 0..6 {
                                    bars[i] = pcie.read_bar(bus, dev, func, i as u8).unwrap_or(0);
                                }

                                let dev_info = PcieDeviceInfo {
                                    bus,
                                    dev,
                                    func,
                                    vendor_id,
                                    device_id,
                                    class_code,
                                    subclass,
                                    prog_if,
                                    bars,
                                };

                                info!(
                                    "Found Device: Bus {:02x}, Dev {:02x}, Func {:x} - {:04x}:{:04x} ({})",
                                    bus,
                                    dev,
                                    func,
                                    vendor_id,
                                    device_id,
                                    dev_info.device_type_name()
                                );

                                devices.push(dev_info);
                            }
                        }
                    }
                }
                Err(_) => {
                    // Function 0 doesn't exist, skip this device
                    continue;
                }
            }
        }

        // If no device on this bus and bus > 5, can skip subsequent buses
        // (most systems don't have more than a few buses)
        if !device_found_on_bus && bus > 5 && devices.is_empty() {
            break;
        }
    }

    info!(
        "=== PCIe Bus Scan Complete, Found {} Device(s) ===",
        devices.len()
    );

    devices
}

/// Test if iATU configuration is working properly
pub fn test_iatu() {
    let pcie = DwPcieConfigAccess::new();

    info!("=== Testing iATU Configuration ===");

    // Test 1: Direct access to config window without iATU configuration
    info!("Test 1: Direct config window access (without iATU)");
    let addr = pcie.cfg_window_virt.as_usize() as *const u32;
    let val1 = unsafe { ptr::read_volatile(addr) };
    info!("  Result: {:#010x} (Expected: 0xFFFFFFFF or random)", val1);

    // Test 2: Access specific device after configuring iATU
    info!("Test 2: Access Bus 31, Dev 0, Func 0 after iATU config");
    match pcie.read_vendor_device_id(31, 0, 0) {
        Ok((vendor, device)) => {
            info!(
                "  Success! Vendor: {:#06x}, Device: {:#06x}",
                vendor, device
            );

            // Print iATU configuration
            pcie.dump_iatu_config(0);
        }
        Err(e) => {
            warn!("  Failed to read: {}", e);
        }
    }
}
