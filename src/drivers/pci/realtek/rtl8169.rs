//! RTL8169/RTL8168/RTL8111 Gigabit Ethernet driver implementation (Refactored)

use crate::drivers::pci::realtek::{DeviceType, EthernetAddress, NetBufPtr};
use crate::{TinyError, TinyResult};

use super::common::{DmaBuffer, DmaBufferArray, MmioOps, RealtekCommon};
use super::device_info::RealtekSeries;
use super::kernel_func::UseKernelFunc as KF;
use super::regs::descriptor::*;
use super::regs::rtl8169::*;
use core::ptr::NonNull;

/// RTL8169 constants
pub const NUM_DESC: usize = 128;
pub const BUF_SIZE: usize = 2048;

/// RTL8169/8168 Descriptor
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy)]
pub struct Descriptor {
    pub opts1: u32,
    pub opts2: u32,
    pub addr_low: u32,
    pub addr_high: u32,
}

/// RTL8169/RTL8168/RTL8111 Gigabit Ethernet Driver
pub struct Rtl8169Driver {
    mmio: MmioOps,
    irq: u8,
    mac: EthernetAddress,
    series: RealtekSeries,

    // Transmit ring
    tx_desc: Option<DmaBuffer>,
    tx_buffers: DmaBufferArray<NUM_DESC>,
    tx_cur: usize,

    // Receive ring
    rx_desc: Option<DmaBuffer>,
    rx_buffers: DmaBufferArray<NUM_DESC>,
    rx_cur: usize,
}

impl Rtl8169Driver {
    /// Create a new RTL8169/8168/8111 driver instance
    pub fn new(base_addr: usize, irq: u8, series: RealtekSeries) -> TinyResult<Self> {
        Ok(Self {
            mmio: MmioOps::new(base_addr),
            irq,
            mac: EthernetAddress([0; 6]),
            series,
            tx_desc: None,
            tx_buffers: DmaBufferArray::new(),
            tx_cur: 0,
            rx_desc: None,
            rx_buffers: DmaBufferArray::new(),
            rx_cur: 0,
        })
    }

    /// Initialize the RTL8169/8168/8111 hardware
    pub fn init(&mut self) -> TinyResult {
        log::info!("[RTL8169] Initializing at {:#x}", self.mmio.base_addr());

        // Software reset
        RealtekCommon::software_reset(&self.mmio, CMD, CMD_RST, 100)?;

        // Unlock configuration registers
        self.mmio.write8(CFG_9346, CFG_9346_UNLOCK);

        // Allocate descriptor rings
        let desc_size = core::mem::size_of::<Descriptor>() * NUM_DESC;
        self.tx_desc = Some(DmaBuffer::alloc(desc_size)?);
        self.rx_desc = Some(DmaBuffer::alloc(desc_size)?);

        // Allocate buffers and setup descriptors
        self.tx_buffers.alloc_all(BUF_SIZE)?;
        self.rx_buffers.alloc_all(BUF_SIZE)?;
        self.setup_tx_ring()?;
        self.setup_rx_ring()?;

        // Configure RCR
        let rcr_val = RCR_AAP | RCR_APM | RCR_AM | RCR_AB | RCR_MXDMA_UNLIMITED | RCR_RXFTH_NONE;
        self.mmio.write32(RCR, rcr_val);

        // Configure TCR
        let tcr_val = TCR_MXDMA_UNLIMITED | TCR_IFG_NORMAL;
        self.mmio.write32(TCR, tcr_val);

        // Set max receive packet size
        self.mmio.write16(RMS, BUF_SIZE as u16);

        // Set early transmit threshold
        self.mmio.write8(ETTHR, 0x3B);

        // Write descriptor ring addresses
        let rx_desc_paddr = self.rx_desc.as_ref().unwrap().paddr();
        let tx_desc_paddr = self.tx_desc.as_ref().unwrap().paddr();
        self.mmio.write32(RDSAR_LO, rx_desc_paddr as u32);
        self.mmio.write32(RDSAR_HI, (rx_desc_paddr >> 32) as u32);
        self.mmio.write32(TNPDS_LO, tx_desc_paddr as u32);
        self.mmio.write32(TNPDS_HI, (tx_desc_paddr >> 32) as u32);

        // Configure interrupts
        let imr_val = INT_ROK | INT_TOK | INT_RER | INT_TER | INT_LINKCHG;
        self.mmio.write16(IMR, imr_val);

        // Enable RX and TX
        self.mmio.write8(CMD, CMD_RE | CMD_TE);

        // Set multicast filter
        self.mmio.write32(MAR0, 0xFFFFFFFF);
        self.mmio.write32(MAR4, 0xFFFFFFFF);

        // Lock configuration registers
        self.mmio.write8(CFG_9346, CFG_9346_LOCK);

        // Read MAC address
        self.mac = RealtekCommon::read_mac_address(&self.mmio, IDR0);
        log::info!(
            "[RTL8169] MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.mac.0[0],
            self.mac.0[1],
            self.mac.0[2],
            self.mac.0[3],
            self.mac.0[4],
            self.mac.0[5]
        );

        Ok(())
    }

    /// Setup transmit ring
    fn setup_tx_ring(&mut self) -> TinyResult {
        let desc_vaddr = self.tx_desc.as_ref().unwrap().vaddr();

        for i in 0..NUM_DESC {
            let desc_ptr = (desc_vaddr + i * core::mem::size_of::<Descriptor>()) as *mut Descriptor;
            let buf_paddr = self.tx_buffers.paddr(i);

            unsafe {
                (*desc_ptr).opts1 = 0;
                (*desc_ptr).opts2 = 0;
                (*desc_ptr).addr_low = buf_paddr as u32;
                (*desc_ptr).addr_high = (buf_paddr >> 32) as u32;
            }
        }
        Ok(())
    }

    /// Setup receive ring
    fn setup_rx_ring(&mut self) -> TinyResult {
        let desc_vaddr = self.rx_desc.as_ref().unwrap().vaddr();

        for i in 0..NUM_DESC {
            let desc_ptr = (desc_vaddr + i * core::mem::size_of::<Descriptor>()) as *mut Descriptor;
            let buf_paddr = self.rx_buffers.paddr(i);

            let opts1 = DESC_OWN | (BUF_SIZE as u32);
            let is_last = i == NUM_DESC - 1;

            unsafe {
                (*desc_ptr).opts1 = if is_last { opts1 | DESC_EOR } else { opts1 };
                (*desc_ptr).opts2 = 0;
                (*desc_ptr).addr_low = buf_paddr as u32;
                (*desc_ptr).addr_high = (buf_paddr >> 32) as u32;
            }
        }
        Ok(())
    }

    /// Get TX descriptor
    #[inline]
    fn tx_desc(&self, idx: usize) -> *mut Descriptor {
        let desc_vaddr = self.tx_desc.as_ref().unwrap().vaddr();
        (desc_vaddr + idx * core::mem::size_of::<Descriptor>()) as *mut Descriptor
    }

    /// Get RX descriptor
    #[inline]
    fn rx_desc(&self, idx: usize) -> *mut Descriptor {
        let desc_vaddr = self.rx_desc.as_ref().unwrap().vaddr();
        (desc_vaddr + idx * core::mem::size_of::<Descriptor>()) as *mut Descriptor
    }

    /// Transmit a packet
    fn do_transmit(&mut self, data: &[u8]) -> TinyResult {
        if data.len() > BUF_SIZE {
            return Err(TinyError::InvalidParameter("Transmit data too large"));
        }

        let idx = self.tx_cur;
        let desc = self.tx_desc(idx);

        // Check if descriptor is available
        let opts1 = unsafe { (*desc).opts1 };
        if (opts1 & DESC_OWN) != 0 {
            return Err(TinyError::Again);
        }

        // Copy data to TX buffer
        let tx_vaddr = self.tx_buffers.vaddr(idx);
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), tx_vaddr as *mut u8, data.len());
        }

        // Setup descriptor and transmit
        let len = RealtekCommon::pad_frame_size(data.len());
        let is_first = idx == 0;
        let is_last = idx == NUM_DESC - 1;

        let mut opts = DESC_OWN | DESC_FS | DESC_LS | (len as u32);
        if is_first {
            opts |= DESC_EOR;
        }
        if is_last {
            opts |= DESC_EOR;
        }

        unsafe {
            (*desc).opts1 = opts;
        }

        // Poll transmit
        self.mmio.write8(TPPOLL, TPPOLL_NPQ);

        // Move to next descriptor
        self.tx_cur = (self.tx_cur + 1) % NUM_DESC;

        Ok(())
    }

    /// Receive a packet
    fn do_receive(&mut self) -> TinyResult<NetBufPtr> {
        let idx = self.rx_cur;
        let desc = self.rx_desc(idx);

        // Check if packet available
        let opts1 = unsafe { (*desc).opts1 };
        if (opts1 & DESC_OWN) != 0 {
            return Err(TinyError::Again);
        }

        // Check for errors
        if (opts1 & (DESC_RX_RES | DESC_RX_RWMA | DESC_RX_RWT | DESC_RX_RUNT | DESC_RX_LONG)) != 0 {
            log::warn!("[RTL8169] RX error, opts1={:#x}", opts1);
            // Reset descriptor
            unsafe {
                (*desc).opts1 = DESC_OWN | (BUF_SIZE as u32);
            }
            self.rx_cur = (self.rx_cur + 1) % NUM_DESC;
            return Err(TinyError::IoError);
        }

        let packet_len = (opts1 & 0x3FFF) as usize;

        // Allocate buffer and copy packet
        let pages = RealtekCommon::pages_for_size(packet_len);
        let (pkt_vaddr, _pkt_paddr) = KF::dma_alloc_coherent(pages);
        if pkt_vaddr == 0 {
            return Err(TinyError::NoMemory);
        }

        let rx_vaddr = self.rx_buffers.vaddr(idx);
        unsafe {
            core::ptr::copy_nonoverlapping(rx_vaddr as *const u8, pkt_vaddr as *mut u8, packet_len);
        }

        // Reset descriptor
        let is_last = idx == NUM_DESC - 1;
        unsafe {
            (*desc).opts1 = DESC_OWN | (BUF_SIZE as u32) | if is_last { DESC_EOR } else { 0 };
        }

        // Move to next descriptor
        self.rx_cur = (self.rx_cur + 1) % NUM_DESC;

        let raw_ptr = NonNull::new(pkt_vaddr as *mut u8).unwrap();
        Ok(NetBufPtr::new(raw_ptr, raw_ptr, packet_len))
    }

    pub fn get_irq(&self) -> u8 {
        self.irq
    }
}

impl super::BaseDriverOps for Rtl8169Driver {
    fn device_name(&self) -> &str {
        match self.series {
            RealtekSeries::Rtl8169 => "rtl8169",
            RealtekSeries::Rtl8168 => "rtl8168",
            RealtekSeries::Rtl8111 => "rtl8111",
            _ => "rtl8169-series",
        }
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Net
    }
}

impl super::NetDriverOps for Rtl8169Driver {
    fn mac_address(&self) -> EthernetAddress {
        self.mac
    }

    fn can_transmit(&self) -> bool {
        let desc = self.tx_desc(self.tx_cur);
        let opts1 = unsafe { (*desc).opts1 };
        (opts1 & DESC_OWN) == 0
    }

    fn can_receive(&self) -> bool {
        let desc = self.rx_desc(self.rx_cur);
        let opts1 = unsafe { (*desc).opts1 };
        (opts1 & DESC_OWN) == 0
    }

    fn rx_queue_size(&self) -> usize {
        NUM_DESC
    }

    fn tx_queue_size(&self) -> usize {
        NUM_DESC
    }

    fn recycle_rx_buffer(&mut self, rx_buf: NetBufPtr) -> TinyResult {
        let vaddr = rx_buf.raw_ptr::<u8>() as usize;
        let pages = RealtekCommon::pages_for_size(rx_buf.packet_len());
        KF::dma_free_coherent(vaddr, pages);
        Ok(())
    }

    fn recycle_tx_buffers(&mut self) -> TinyResult {
        Ok(()) // Descriptors are reused automatically
    }

    fn transmit(&mut self, tx_buf: NetBufPtr) -> TinyResult {
        let data = tx_buf.packet();
        self.do_transmit(data)
    }

    fn receive(&mut self) -> TinyResult<NetBufPtr> {
        self.do_receive()
    }

    fn alloc_tx_buffer(&mut self, size: usize) -> TinyResult<NetBufPtr> {
        if size > BUF_SIZE {
            return Err(TinyError::InvalidParameter(
                "Requested TX buffer size too large",
            ));
        }

        let pages = RealtekCommon::pages_for_size(size);
        let (vaddr, _paddr) = KF::dma_alloc_coherent(pages);

        if vaddr == 0 {
            return Err(TinyError::NoMemory);
        }

        let raw_ptr = NonNull::new(vaddr as *mut u8).unwrap();
        Ok(NetBufPtr::new(raw_ptr, raw_ptr, size))
    }
}
