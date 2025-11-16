//! RTL8125 2.5G Ethernet Controller Driver
//!
//! This driver supports the Realtek RTL8125 2.5 Gigabit Ethernet controller.
//! It is based on the U-Boot rtl8169 driver but optimized for RTL8125.

use crate::hal::cpu::{clean_dcache_range, invalidate_dcache_range};
use crate::mm::{phys_to_virt, virt_to_phys};
use core::ptr::{read_volatile, write_volatile};
use memory_addr::{PhysAddr, VirtAddr};

/// RTL8125 register definitions
#[repr(u32)]
#[allow(dead_code)]
enum Rtl8125Registers {
    MAC0 = 0x00, // Ethernet hardware address
    MAR0 = 0x08, // Multicast filter
    TxDescStartAddrLow = 0x20,
    TxDescStartAddrHigh = 0x24,
    TxHDescStartAddrLow = 0x28,
    TxHDescStartAddrHigh = 0x2c,
    FLASH = 0x30,
    ERSR = 0x36,
    ChipCmd = 0x37,
    IntrMask = 0x38,   // RTL8125 specific
    IntrStatus = 0x3C, // RTL8125 specific
    TxConfig = 0x40,
    RxConfig = 0x44,
    RxMissed = 0x4C,
    Cfg9346 = 0x50,
    Config0 = 0x51,
    Config1 = 0x52,
    Config2 = 0x53,
    Config3 = 0x54,
    Config4 = 0x55,
    Config5 = 0x56,
    MultiIntr = 0x5C,
    PHYAR = 0x60,
    TBICSR = 0x64,
    TBI_ANAR = 0x68,
    TBI_LPAR = 0x6A,
    PHYstatus = 0x6C,
    TxPoll = 0x90, // RTL8125 specific
    RxMaxSize = 0xDA,
    CPlusCmd = 0xE0,
    RxDescStartAddrLow = 0xE4,
    RxDescStartAddrHigh = 0xE8,
    EarlyTxThres = 0xEC,
    FuncEvent = 0xF0,
    FuncEventMask = 0xF4,
    FuncPresetState = 0xF8,
    FuncForceEvent = 0xFC,
}

/// Register content bit definitions
#[allow(dead_code)]
mod register_bits {
    // Interrupt Status Bits
    pub const SYS_ERR: u16 = 0x8000;
    pub const PCS_TIMEOUT: u16 = 0x4000;
    pub const SW_INT: u16 = 0x0100;
    pub const TX_DESC_UNAVAIL: u16 = 0x0080;
    pub const RX_FIFO_OVER: u16 = 0x0040;
    pub const RX_UNDERRUN: u16 = 0x0020;
    pub const RX_OVERFLOW: u16 = 0x0010;
    pub const TX_ERR: u16 = 0x0008;
    pub const TX_OK: u16 = 0x0004;
    pub const RX_ERR: u16 = 0x0002;
    pub const RX_OK: u16 = 0x0001;

    // RxStatusDesc
    pub const RX_RES: u32 = 0x00200000;
    pub const RX_CRC: u32 = 0x00080000;
    pub const RX_RUNT: u32 = 0x00100000;
    pub const RX_RWT: u32 = 0x00400000;

    // ChipCmd bits
    pub const CMD_RESET: u8 = 0x10;
    pub const CMD_RX_ENB: u8 = 0x08;
    pub const CMD_TX_ENB: u8 = 0x04;
    pub const RX_BUF_EMPTY: u8 = 0x01;

    // Cfg9346 bits
    pub const CFG9346_LOCK: u8 = 0x00;
    pub const CFG9346_UNLOCK: u8 = 0xC0;

    // rx_mode bits
    pub const ACCEPT_ERR: u32 = 0x20;
    pub const ACCEPT_RUNT: u32 = 0x10;
    pub const ACCEPT_BROADCAST: u32 = 0x08;
    pub const ACCEPT_MULTICAST: u32 = 0x04;
    pub const ACCEPT_MY_PHYS: u32 = 0x02;
    pub const ACCEPT_ALL_PHYS: u32 = 0x01;

    // PHY registers
    pub const PHY_CTRL_REG: u8 = 0;
    pub const PHY_STAT_REG: u8 = 1;
    pub const PHY_AUTO_NEGO_REG: u8 = 4;
    pub const PHY_1000_CTRL_REG: u8 = 9;

    // PHY bits
    pub const PHY_RESTART_AUTO_NEGO: u16 = 0x0200;
    pub const PHY_ENABLE_AUTO_NEGO: u16 = 0x1000;
    pub const PHY_AUTO_NEGO_COMP: u16 = 0x0020;

    // PHY capabilities
    pub const PHY_CAP_10_HALF: u16 = 0x0020;
    pub const PHY_CAP_10_FULL: u16 = 0x0040;
    pub const PHY_CAP_100_HALF: u16 = 0x0080;
    pub const PHY_CAP_100_FULL: u16 = 0x0100;
    pub const PHY_CAP_1000_FULL: u16 = 0x0200;

    // PHYstatus
    pub const LINK_STATUS: u8 = 0x02;
    pub const FULL_DUP: u8 = 0x01;
    pub const _1000BPSF: u8 = 0x10;
    pub const _100BPS: u8 = 0x08;
    pub const _10BPS: u8 = 0x04;

    // FuncEvent/Misc
    pub const RX_DV_GATED_EN: u32 = 0x80000;
}

/// Configuration constants
const NUM_TX_DESC: usize = 1;
const NUM_RX_DESC: usize = 4;
const RX_BUF_SIZE: usize = 1536;
const MAX_ETH_FRAME_SIZE: usize = 1536;
const RX_FIFO_THRESH: u32 = 7;
const RX_DMA_BURST: u32 = 6;
const TX_DMA_BURST: u32 = 6;
const EARLY_TX_THLD: u8 = 0x3F;
const RX_PACKET_MAX_SIZE: u16 = 0x0800;
const INTER_FRAME_GAP: u32 = 0x03;
const TX_TIMEOUT: usize = 6000; // milliseconds

/// Descriptor status bits
#[allow(dead_code)]
mod desc_status {
    pub const OWN: u32 = 0x80000000;
    pub const EOR: u32 = 0x40000000;
    pub const FS: u32 = 0x20000000;
    pub const LS: u32 = 0x10000000;
}

/// TX Descriptor
#[repr(C, align(256))]
#[derive(Debug, Clone, Copy)]
struct TxDesc {
    status: u32,
    vlan_tag: u32,
    buf_addr: u32,
    buf_haddr: u32,
}

impl TxDesc {
    const fn new() -> Self {
        Self {
            status: 0,
            vlan_tag: 0,
            buf_addr: 0,
            buf_haddr: 0,
        }
    }
}

/// RX Descriptor
#[repr(C, align(256))]
#[derive(Debug, Clone, Copy)]
struct RxDesc {
    status: u32,
    vlan_tag: u32,
    buf_addr: u32,
    buf_haddr: u32,
}

impl RxDesc {
    const fn new() -> Self {
        Self {
            status: 0,
            vlan_tag: 0,
            buf_addr: 0,
            buf_haddr: 0,
        }
    }
}

/// RTL8125 device driver
pub struct Rtl8125 {
    /// MMIO base address (virtual)
    iobase: usize,
    /// Current RX descriptor index
    cur_rx: usize,
    /// Current TX descriptor index
    cur_tx: usize,
    /// TX descriptors
    tx_desc: [TxDesc; NUM_TX_DESC],
    /// RX descriptors
    rx_desc: [RxDesc; NUM_RX_DESC],
    /// TX buffers
    tx_buf: [[u8; RX_BUF_SIZE]; NUM_TX_DESC],
    /// RX buffers
    rx_buf: [[u8; RX_BUF_SIZE]; NUM_RX_DESC],
    /// MAC address
    mac_addr: [u8; 6],
}

impl Rtl8125 {
    /// Create a new RTL8125 driver instance
    pub fn new(mmio_base: PhysAddr) -> Self {
        let iobase = phys_to_virt(mmio_base).as_usize();

        Self {
            iobase,
            cur_rx: 0,
            cur_tx: 0,
            tx_desc: [TxDesc::new(); NUM_TX_DESC],
            rx_desc: [RxDesc::new(); NUM_RX_DESC],
            tx_buf: [[0u8; RX_BUF_SIZE]; NUM_TX_DESC],
            rx_buf: [[0u8; RX_BUF_SIZE]; NUM_RX_DESC],
            mac_addr: [0u8; 6],
        }
    }

    /// Read 8-bit register
    #[inline]
    fn read8(&self, reg: Rtl8125Registers) -> u8 {
        unsafe { read_volatile((self.iobase + reg as usize) as *const u8) }
    }

    /// Write 8-bit register
    #[inline]
    fn write8(&mut self, reg: Rtl8125Registers, val: u8) {
        unsafe { write_volatile((self.iobase + reg as usize) as *mut u8, val) }
    }

    /// Read 16-bit register
    #[inline]
    fn read16(&self, reg: Rtl8125Registers) -> u16 {
        unsafe { read_volatile((self.iobase + reg as usize) as *const u16) }
    }

    /// Write 16-bit register
    #[inline]
    fn write16(&mut self, reg: Rtl8125Registers, val: u16) {
        unsafe { write_volatile((self.iobase + reg as usize) as *mut u16, val) }
    }

    /// Read 32-bit register
    #[inline]
    fn read32(&self, reg: Rtl8125Registers) -> u32 {
        unsafe { read_volatile((self.iobase + reg as usize) as *const u32) }
    }

    /// Write 32-bit register
    #[inline]
    fn write32(&mut self, reg: Rtl8125Registers, val: u32) {
        unsafe { write_volatile((self.iobase + reg as usize) as *mut u32, val) }
    }

    /// Delay for microseconds using hardware timer
    fn udelay(&self, us: usize) {
        crate::drivers::timer::busy_wait(core::time::Duration::from_micros(us as u64));
    }

    /// Read MII register via MDIO
    fn mdio_read(&mut self, reg_addr: u8) -> u16 {
        self.write32(Rtl8125Registers::PHYAR, ((reg_addr as u32) << 16));
        self.udelay(1000);

        for _ in 0..2000 {
            let val = self.read32(Rtl8125Registers::PHYAR);
            if (val & 0x80000000) != 0 {
                return (val & 0xFFFF) as u16;
            }
            self.udelay(100);
        }
        0xFFFF
    }

    /// Write MII register via MDIO
    fn mdio_write(&mut self, reg_addr: u8, value: u16) {
        self.write32(
            Rtl8125Registers::PHYAR,
            0x80000000 | ((reg_addr as u32) << 16) | (value as u32),
        );
        self.udelay(1000);

        for _ in 0..2000 {
            if (self.read32(Rtl8125Registers::PHYAR) & 0x80000000) == 0 {
                break;
            }
            self.udelay(100);
        }
    }

    /// Initialize the RTL8125 chip
    pub fn init(&mut self) -> Result<(), &'static str> {
        info!("RTL8125: Initializing...");

        // Soft reset the chip
        self.write8(Rtl8125Registers::ChipCmd, register_bits::CMD_RESET);

        // Wait for reset to complete
        for _ in 0..1000 {
            if (self.read8(Rtl8125Registers::ChipCmd) & register_bits::CMD_RESET) == 0 {
                break;
            }
            self.udelay(10);
        }

        // Read MAC address
        for i in 0..6 {
            self.mac_addr[i] = unsafe {
                read_volatile((self.iobase + Rtl8125Registers::MAC0 as usize + i) as *const u8)
            };
        }
        info!(
            "RTL8125: MAC address: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.mac_addr[0],
            self.mac_addr[1],
            self.mac_addr[2],
            self.mac_addr[3],
            self.mac_addr[4],
            self.mac_addr[5]
        );

        // Check if TBI is enabled
        if (self.read8(Rtl8125Registers::PHYstatus) & 0x80) == 0 {
            self.init_phy()?;
        }

        // Initialize descriptors
        self.init_ring();

        // Start hardware
        self.hw_start();

        info!("RTL8125: Initialization complete");
        Ok(())
    }

    /// Initialize PHY for auto-negotiation
    fn init_phy(&mut self) -> Result<(), &'static str> {
        info!("RTL8125: Initializing PHY for auto-negotiation");

        let val = self.mdio_read(register_bits::PHY_AUTO_NEGO_REG);

        // Enable 10/100 Full/Half Mode
        self.mdio_write(
            register_bits::PHY_AUTO_NEGO_REG,
            register_bits::PHY_CAP_10_HALF
                | register_bits::PHY_CAP_10_FULL
                | register_bits::PHY_CAP_100_HALF
                | register_bits::PHY_CAP_100_FULL
                | (val & 0x1F),
        );

        // Enable 1000 Full Mode
        self.mdio_write(
            register_bits::PHY_1000_CTRL_REG,
            register_bits::PHY_CAP_1000_FULL,
        );

        // Enable auto-negotiation and restart
        self.mdio_write(
            register_bits::PHY_CTRL_REG,
            register_bits::PHY_ENABLE_AUTO_NEGO | register_bits::PHY_RESTART_AUTO_NEGO,
        );
        self.udelay(100);

        // Wait for auto-negotiation to complete
        for _ in 0..10000 {
            if (self.mdio_read(register_bits::PHY_STAT_REG) & register_bits::PHY_AUTO_NEGO_COMP)
                != 0
            {
                self.udelay(100);
                let status = self.read8(Rtl8125Registers::PHYstatus);

                if (status & register_bits::_1000BPSF) != 0 {
                    info!("RTL8125: Link up at 1000Mbps Full-duplex");
                } else if (status & register_bits::_100BPS) != 0 {
                    info!(
                        "RTL8125: Link up at 100Mbps {}-duplex",
                        if (status & register_bits::FULL_DUP) != 0 {
                            "Full"
                        } else {
                            "Half"
                        }
                    );
                } else {
                    info!(
                        "RTL8125: Link up at 10Mbps {}-duplex",
                        if (status & register_bits::FULL_DUP) != 0 {
                            "Full"
                        } else {
                            "Half"
                        }
                    );
                }
                return Ok(());
            }
            self.udelay(100);
        }

        warn!("RTL8125: Auto-negotiation timeout");
        Ok(())
    }

    /// Initialize TX and RX descriptor rings
    fn init_ring(&mut self) {
        info!("RTL8125: Initializing descriptor rings");

        self.cur_rx = 0;
        self.cur_tx = 0;

        // Clear all descriptors to zero first (like memset in rtl8169.c)
        self.tx_desc = [TxDesc::new(); NUM_TX_DESC];
        self.rx_desc = [RxDesc::new(); NUM_RX_DESC];

        // Initialize TX descriptors with buffer addresses
        for i in 0..NUM_TX_DESC {
            let tx_buf_vaddr = self.tx_buf[i].as_ptr() as usize;
            let tx_buf_paddr = virt_to_phys(VirtAddr::from(tx_buf_vaddr)).as_usize();

            // Pre-setup TX descriptor buffer address (not status yet, set on send)
            self.tx_desc[i].buf_addr = tx_buf_paddr as u32;
            self.tx_desc[i].buf_haddr = 0;

            info!(
                "RTL8125: TX buffer[{}] vaddr: {:#x}, paddr: {:#x}",
                i, tx_buf_vaddr, tx_buf_paddr
            );
        }

        // Initialize RX descriptors
        for i in 0..NUM_RX_DESC {
            let rx_buf_vaddr = self.rx_buf[i].as_ptr() as usize;
            let rx_buf_paddr = virt_to_phys(VirtAddr::from(rx_buf_vaddr)).as_usize();

            // Set descriptor fields (order doesn't matter before cache flush)
            // Hardware won't see any of this until we clean_dcache below
            let status = if i == NUM_RX_DESC - 1 {
                desc_status::OWN | desc_status::EOR | RX_BUF_SIZE as u32
            } else {
                desc_status::OWN | RX_BUF_SIZE as u32
            };
            self.rx_desc[i].status = status;
            self.rx_desc[i].vlan_tag = 0;
            self.rx_desc[i].buf_addr = rx_buf_paddr as u32;
            self.rx_desc[i].buf_haddr = 0;

            info!(
                "RTL8125: RX buffer[{}] vaddr: {:#x}, paddr: {:#x}",
                i, rx_buf_vaddr, rx_buf_paddr
            );
        }

        // Critical: Flush descriptor rings to memory so hardware can see them
        info!("RTL8125: Flushing TX/RX descriptor rings to memory");
        unsafe {
            clean_dcache_range(
                self.tx_desc.as_ptr() as usize,
                core::mem::size_of_val(&self.tx_desc),
            );
            clean_dcache_range(
                self.rx_desc.as_ptr() as usize,
                core::mem::size_of_val(&self.rx_desc),
            );
        }

        // CRITICAL: Invalidate all RX buffers BEFORE starting hardware!
        // This ensures CPU cache doesn't contain stale data that would be read
        // instead of the actual packet data written by hardware DMA.
        info!("RTL8125: Invalidating RX buffers to prevent stale cache data");
        for i in 0..NUM_RX_DESC {
            unsafe {
                invalidate_dcache_range(self.rx_buf[i].as_ptr() as usize, RX_BUF_SIZE);
            }
        }
    }

    /// Start the hardware
    fn hw_start(&mut self) {
        info!("RTL8125: Starting hardware");

        // Unlock config registers
        self.write8(Rtl8125Registers::Cfg9346, register_bits::CFG9346_UNLOCK);

        // Set early TX threshold
        self.write8(Rtl8125Registers::EarlyTxThres, EARLY_TX_THLD);

        // Set RX max size
        self.write16(Rtl8125Registers::RxMaxSize, RX_PACKET_MAX_SIZE);

        // Set RX config with accept flags (CRITICAL: must include RX mode bits)
        // C version: (7 << 13) | (6 << 8) | 0x0E = 0x0000e60e
        // 0x0E = ACCEPT_BROADCAST | ACCEPT_MULTICAST | ACCEPT_MY_PHYS
        let rx_config = (RX_FIFO_THRESH << 13) | (RX_DMA_BURST << 8) | 0x0E;
        self.write32(Rtl8125Registers::RxConfig, rx_config);
        info!("RTL8125: RX Config: 0x{:08x}", rx_config);

        // Set TX config
        let tx_config = (TX_DMA_BURST << 8) | (INTER_FRAME_GAP << 24);
        self.write32(Rtl8125Registers::TxConfig, tx_config);
        info!("RTL8125: TX Config: 0x{:08x}", tx_config);

        // Set descriptor addresses (must use physical addresses for DMA)
        let tx_desc_paddr = virt_to_phys(VirtAddr::from(self.tx_desc.as_ptr() as usize)).as_usize();
        let rx_desc_paddr = virt_to_phys(VirtAddr::from(self.rx_desc.as_ptr() as usize)).as_usize();

        info!("RTL8125: TX desc paddr: {:#x}", tx_desc_paddr);
        info!("RTL8125: RX desc paddr: {:#x}", rx_desc_paddr);

        self.write32(Rtl8125Registers::TxDescStartAddrLow, tx_desc_paddr as u32);
        self.write32(Rtl8125Registers::TxDescStartAddrHigh, 0);
        self.write32(Rtl8125Registers::RxDescStartAddrLow, rx_desc_paddr as u32);
        self.write32(Rtl8125Registers::RxDescStartAddrHigh, 0);

        // Enable TX and RX
        self.write8(
            Rtl8125Registers::ChipCmd,
            register_bits::CMD_TX_ENB | register_bits::CMD_RX_ENB,
        );

        // Lock config registers
        self.write8(Rtl8125Registers::Cfg9346, register_bits::CFG9346_LOCK);
        self.udelay(10);

        // Clear RX missed counter
        self.write32(Rtl8125Registers::RxMissed, 0);

        // Set RX mode
        self.set_rx_mode();

        // Clear multi-interrupt
        let multi_intr = self.read16(Rtl8125Registers::MultiIntr);
        self.write16(Rtl8125Registers::MultiIntr, multi_intr & 0xF000);

        // WAR: Clear RxDv_Gated_En bit
        let func_event = self.read32(Rtl8125Registers::FuncEvent);
        self.write32(
            Rtl8125Registers::FuncEvent,
            func_event & !register_bits::RX_DV_GATED_EN,
        );
    }

    /// Set RX mode to accept broadcast, multicast, and our packets
    fn set_rx_mode(&mut self) {
        let rx_mode = register_bits::ACCEPT_BROADCAST
            | register_bits::ACCEPT_MULTICAST
            | register_bits::ACCEPT_MY_PHYS;

        let rx_config = self.read32(Rtl8125Registers::RxConfig);
        self.write32(
            Rtl8125Registers::RxConfig,
            (rx_config & 0xFF7E1880) | rx_mode,
        );

        // Set multicast filter to accept all
        self.write32(Rtl8125Registers::MAR0, 0xFFFFFFFF);
        unsafe {
            write_volatile(
                (self.iobase + Rtl8125Registers::MAR0 as usize + 4) as *mut u32,
                0xFFFFFFFF,
            );
        }
    }

    /// Send a packet
    pub fn send(&mut self, data: &[u8]) -> Result<(), &'static str> {
        if data.len() > MAX_ETH_FRAME_SIZE {
            return Err("Packet too large");
        }

        let entry = self.cur_tx % NUM_TX_DESC;
        let len = if data.len() < 60 { 60 } else { data.len() };

        info!("RTL8125: send() called, entry={}, len={}, cur_tx={}", entry, len, self.cur_tx);

        // Copy data to TX buffer
        self.tx_buf[entry][..data.len()].copy_from_slice(data);

        // Zero-pad if necessary
        if data.len() < 60 {
            self.tx_buf[entry][data.len()..60].fill(0);
        }

        // Critical: Flush TX buffer cache so hardware can see the data
        unsafe {
            clean_dcache_range(self.tx_buf[entry].as_ptr() as usize, len);
        }

        // Set up TX descriptor status (buf_addr already set in init_ring)
        let status = if entry == NUM_TX_DESC - 1 {
            desc_status::OWN | desc_status::EOR | desc_status::FS | desc_status::LS | len as u32
        } else {
            desc_status::OWN | desc_status::FS | desc_status::LS | len as u32
        };

        // Note: buf_addr and buf_haddr are already set in init_ring()
        // Just update the status to trigger transmission
        self.tx_desc[entry].status = status;

        // Critical: Flush TX descriptor cache so hardware can see it
        unsafe {
            clean_dcache_range(
                &self.tx_desc[entry] as *const _ as usize,
                core::mem::size_of::<TxDesc>(),
            );
        }

        // Trigger TX poll
        self.write8(Rtl8125Registers::TxPoll, 0x1);

        self.cur_tx += 1;

        // Wait for transmission to complete
        let _start = 0; // In real implementation, use timer
        for _ in 0..TX_TIMEOUT {
            // Critical: Invalidate descriptor cache to read hardware updates
            unsafe {
                invalidate_dcache_range(
                    &self.tx_desc[entry] as *const _ as usize,
                    core::mem::size_of::<TxDesc>(),
                );
            }

            if (self.tx_desc[entry].status & desc_status::OWN) == 0 {
                self.udelay(20); // Delay for net console
                return Ok(());
            }
            self.udelay(1000);
        }

        Err("TX timeout")
    }

    /// Receive a packet
    pub fn recv(&mut self, buf: &mut [u8]) -> Result<usize, &'static str> {
        // Try all RX descriptors, starting from cur_rx
        // Hardware may not use descriptors in the order we expect
        for i in 0..NUM_RX_DESC {
            let idx = (self.cur_rx + i) % NUM_RX_DESC;

            // Critical: Invalidate descriptor cache to read hardware updates
            unsafe {
                invalidate_dcache_range(
                    &self.rx_desc[idx] as *const _ as usize,
                    core::mem::size_of::<RxDesc>(),
                );
            }

            let rx_status = self.rx_desc[idx].status;

            // Check if this descriptor has a packet (OWN bit clear)
            if (rx_status & desc_status::OWN) != 0 {
                // No packet in this descriptor, try next
                continue;
            }

            // Found a packet! Process it at descriptor idx
            let cur_rx = idx;
            info!("RTL8125: recv() found packet at RX[{}], status=0x{:08x}", cur_rx, rx_status);

            // Check for RX errors
            if (rx_status & register_bits::RX_RES) != 0 {
                error!("RTL8125: RX error detected");

                // Get buffer addresses for re-initialization
                let rx_buf_vaddr = self.rx_buf[cur_rx].as_ptr() as usize;
                let rx_buf_paddr = virt_to_phys(VirtAddr::from(rx_buf_vaddr)).as_usize();

                // Reclaim descriptor with re-initialization of buf_addr (critical fix)
                let status = if cur_rx == NUM_RX_DESC - 1 {
                    desc_status::OWN | desc_status::EOR | RX_BUF_SIZE as u32
                } else {
                    desc_status::OWN | RX_BUF_SIZE as u32
                };

                // Critical: Re-write buffer address (like C version does)
                self.rx_desc[cur_rx].buf_addr = rx_buf_paddr as u32;
                self.rx_desc[cur_rx].buf_haddr = 0;
                self.rx_desc[cur_rx].status = status;

                // Critical: Flush descriptor cache so hardware can see updates
                unsafe {
                    clean_dcache_range(
                        &self.rx_desc[cur_rx] as *const _ as usize,
                        core::mem::size_of::<RxDesc>(),
                    );
                }

                self.cur_rx = (cur_rx + 1) % NUM_RX_DESC;
                return Err("RX error");
            }

            // Get packet length (minus CRC)
            let len = ((rx_status & 0x1FFF) - 4) as usize;
            info!("RTL8125: RX packet length: {} bytes", len);

            if len > buf.len() {
                return Err("Buffer too small");
            }

            // Critical: Invalidate ENTIRE RX buffer cache to read hardware-written data
            unsafe {
                invalidate_dcache_range(self.rx_buf[cur_rx].as_ptr() as usize, RX_BUF_SIZE);
            }

            // Copy data to user buffer
            buf[..len].copy_from_slice(&self.rx_buf[cur_rx][..len]);

            // Get buffer addresses for re-initialization
            let rx_buf_vaddr = self.rx_buf[cur_rx].as_ptr() as usize;
            let rx_buf_paddr = virt_to_phys(VirtAddr::from(rx_buf_vaddr)).as_usize();

            // Reclaim descriptor for reuse
            let status = if cur_rx == NUM_RX_DESC - 1 {
                desc_status::OWN | desc_status::EOR | RX_BUF_SIZE as u32
            } else {
                desc_status::OWN | RX_BUF_SIZE as u32
            };

            self.rx_desc[cur_rx].buf_addr = rx_buf_paddr as u32;
            self.rx_desc[cur_rx].buf_haddr = 0;
            self.rx_desc[cur_rx].status = status;

            // Critical: Flush descriptor cache so hardware can see updates
            unsafe {
                clean_dcache_range(
                    &self.rx_desc[cur_rx] as *const _ as usize,
                    core::mem::size_of::<RxDesc>(),
                );
            }

            self.cur_rx = (cur_rx + 1) % NUM_RX_DESC;

            return Ok(len);
        }

        // No packet available in any descriptor
        Err("No packet available")
    }

    /// Get MAC address
    pub fn mac_address(&self) -> [u8; 6] {
        self.mac_addr
    }

    /// Stop the device
    pub fn stop(&mut self) {
        info!("RTL8125: Stopping device");

        // Stop TX and RX
        self.write8(Rtl8125Registers::ChipCmd, 0x00);

        // Disable interrupts
        self.write16(Rtl8125Registers::IntrMask, 0x0000);

        // Clear RX missed counter
        self.write32(Rtl8125Registers::RxMissed, 0);
    }
}

/// Test function for RTL8125
pub fn test_rtl8125(mmio_base: PhysAddr) {
    info!("=== Testing RTL8125 Driver ===");

    let mut rtl = Rtl8125::new(mmio_base);

    match rtl.init() {
        Ok(_) => {
            info!("RTL8125: Device initialized successfully");

            // Example: Send a test packet (ARP request or similar)
            // In a real implementation, you would integrate with a network stack
        }
        Err(e) => {
            error!("RTL8125: Failed to initialize: {}", e);
        }
    }
}
