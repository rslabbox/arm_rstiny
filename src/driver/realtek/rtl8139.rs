//! RTL8139 Fast Ethernet driver implementation (Refactored)

use super::common::{DmaBuffer, DmaBufferArray, MmioOps, RealtekCommon};
use super::kernel_func::UseKernelFunc as KF;
use super::regs::rtl8139::*;
use super::{BaseDriverOps, DevError, DevResult, DeviceType};
use super::{EthernetAddress, NetBufPtr, NetDriverOps};
use core::ptr::NonNull;

/// RTL8139 constants
pub const NUM_TX_DESC: usize = 4;
pub const TX_BUF_SIZE: usize = 2048;
pub const RX_BUF_SIZE: usize = 8192 + 16 + 1536; // 8KB + margin

/// RTL8139 Fast Ethernet Driver
pub struct Rtl8139Driver {
    mmio: MmioOps,
    irq: u8,
    mac: EthernetAddress,

    // Transmit state
    tx_cur: usize,
    tx_buffers: DmaBufferArray<NUM_TX_DESC>,

    // Receive state
    rx_buffer: Option<DmaBuffer>,
    cur_rx: usize,
}

impl Rtl8139Driver {
    /// Create a new RTL8139 driver instance
    pub fn new(base_addr: usize, irq: u8) -> DevResult<Self> {
        Ok(Self {
            mmio: MmioOps::new(base_addr),
            irq,
            mac: EthernetAddress([0; 6]),
            tx_cur: 0,
            tx_buffers: DmaBufferArray::new(),
            rx_buffer: None,
            cur_rx: 0,
        })
    }

    /// Initialize the RTL8139 hardware
    pub fn init(&mut self) -> DevResult {
        log::info!("[RTL8139] Initializing at {:#x}", self.mmio.base_addr());

        // Power on and reset
        self.mmio.write8(CONFIG1, 0x00);
        RealtekCommon::software_reset(&self.mmio, CR, CR_RST, 100)?;

        // Allocate buffers
        self.rx_buffer = Some(DmaBuffer::alloc(RX_BUF_SIZE)?);
        self.tx_buffers.alloc_all(TX_BUF_SIZE)?;

        // Enable Tx/Rx
        let cmd_val = (self.mmio.read8(CR) & !0x1C) | CR_TE | CR_RE;
        self.mmio.write8(CR, cmd_val);

        // Configure TCR
        let tcr_val = (6 << 8) | (3 << 24); // DMA burst 1024, normal IFG
        self.mmio.write32(TCR, tcr_val);

        // Configure RCR
        let rcr_val = RCR_AAP | RCR_APM | RCR_AM | RCR_AB | RCR_WRAP;
        self.mmio.write32(RCR, rcr_val);

        // Set TX descriptor addresses
        for i in 0..NUM_TX_DESC {
            let tsad_reg = TSAD0 + (i as u16 * 4);
            self.mmio.write32(tsad_reg, self.tx_buffers.paddr(i) as u32);
        }

        // Set RX buffer address
        let rx_paddr = self.rx_buffer.as_ref().unwrap().paddr();
        self.mmio.write32(RBSTART, rx_paddr as u32);

        // Initialize MPC
        self.mmio.write32(MPC, 0);

        // Configure interrupts
        let imr_val = INT_ROK | INT_TOK | INT_RER | INT_TER | INT_RXOVW;
        self.mmio.write16(IMR, imr_val);

        // Read MAC address
        self.mac = RealtekCommon::read_mac_address(&self.mmio, IDR0);
        log::info!(
            "[RTL8139] MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.mac.0[0],
            self.mac.0[1],
            self.mac.0[2],
            self.mac.0[3],
            self.mac.0[4],
            self.mac.0[5]
        );

        Ok(())
    }

    /// Transmit a packet
    fn do_transmit(&mut self, data: &[u8]) -> DevResult {
        if data.len() > TX_BUF_SIZE {
            return Err(DevError::InvalidParam);
        }

        let idx = self.tx_cur;

        // Check if descriptor is available
        let status = self.mmio.read32(TSD0 + (idx as u16 * 4));
        if (status & (TSD_OWN | TSD_TOK)) == 0 {
            return Err(DevError::Again);
        }

        // Copy data to TX buffer
        let tx_vaddr = self.tx_buffers.vaddr(idx);
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), tx_vaddr as *mut u8, data.len());
        }

        // Pad to minimum size and transmit
        let len = RealtekCommon::pad_frame_size(data.len());
        self.mmio.write32(TSD0 + (idx as u16 * 4), len as u32);

        // Move to next descriptor
        self.tx_cur = (self.tx_cur + 1) % NUM_TX_DESC;

        Ok(())
    }

    /// Receive a packet
    fn do_receive(&mut self) -> DevResult<NetBufPtr> {
        // Check if packet available
        let cmd = self.mmio.read8(CR);
        if (cmd & CR_BUFE) != 0 {
            return Err(DevError::Again); // Buffer empty
        }

        let rx_vaddr = self.rx_buffer.as_ref().unwrap().vaddr();
        let cur_offset = self.cur_rx % RX_BUF_SIZE;
        let header_ptr = (rx_vaddr + cur_offset) as *const u32;
        let header = unsafe { core::ptr::read_volatile(header_ptr) };

        // Parse header
        let status = (header & 0xFFFF) as u16;
        let length = ((header >> 16) & 0xFFFF) as usize;

        if (status & RX_ROK) == 0 {
            log::warn!("[RTL8139] RX error, status={:#x}", status);
            self.cur_rx = (self.cur_rx + length + 4 + 3) & !3;
            return Err(DevError::Io);
        }

        let packet_len = length - 4; // Exclude CRC

        // Allocate buffer and copy packet
        let pages = RealtekCommon::pages_for_size(packet_len);
        let (pkt_vaddr, _pkt_paddr) = KF::dma_alloc_coherent(pages);
        if pkt_vaddr == 0 {
            return Err(DevError::NoMemory);
        }

        let data_offset = (cur_offset + 4) % RX_BUF_SIZE;
        let src_ptr = (rx_vaddr + data_offset) as *const u8;
        unsafe {
            core::ptr::copy_nonoverlapping(src_ptr, pkt_vaddr as *mut u8, packet_len);
        }

        // Update CAPR
        self.cur_rx = (self.cur_rx + length + 4 + 3) & !3;
        self.mmio.write16(CAPR, (self.cur_rx - 16) as u16);

        let raw_ptr = NonNull::new(pkt_vaddr as *mut u8).unwrap();
        Ok(NetBufPtr::new(raw_ptr, raw_ptr, packet_len))
    }

    pub fn get_irq(&self) -> u8 {
        self.irq
    }
}

impl BaseDriverOps for Rtl8139Driver {
    fn device_name(&self) -> &str {
        "rtl8139"
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Net
    }
}

impl NetDriverOps for Rtl8139Driver {
    fn mac_address(&self) -> EthernetAddress {
        self.mac
    }

    fn can_transmit(&self) -> bool {
        let idx = self.tx_cur;
        let status = self.mmio.read32(TSD0 + (idx as u16 * 4));
        (status & (TSD_OWN | TSD_TOK)) != 0
    }

    fn can_receive(&self) -> bool {
        let cmd = self.mmio.read8(CR);
        (cmd & CR_BUFE) == 0
    }

    fn rx_queue_size(&self) -> usize {
        1 // RTL8139 uses single RX buffer
    }

    fn tx_queue_size(&self) -> usize {
        NUM_TX_DESC
    }

    fn recycle_rx_buffer(&mut self, rx_buf: NetBufPtr) -> DevResult {
        let vaddr = rx_buf.raw_ptr::<u8>() as usize;
        let pages = RealtekCommon::pages_for_size(rx_buf.packet_len());
        KF::dma_free_coherent(vaddr, pages);
        Ok(())
    }

    fn recycle_tx_buffers(&mut self) -> DevResult {
        Ok(()) // No action needed for RTL8139
    }

    fn transmit(&mut self, tx_buf: NetBufPtr) -> DevResult {
        let data = tx_buf.packet();
        self.do_transmit(data)
    }

    fn receive(&mut self) -> DevResult<NetBufPtr> {
        self.do_receive()
    }

    fn alloc_tx_buffer(&mut self, size: usize) -> DevResult<NetBufPtr> {
        if size > TX_BUF_SIZE {
            return Err(DevError::InvalidParam);
        }

        let pages = RealtekCommon::pages_for_size(size);
        let (vaddr, _paddr) = KF::dma_alloc_coherent(pages);

        if vaddr == 0 {
            return Err(DevError::NoMemory);
        }

        let raw_ptr = NonNull::new(vaddr as *mut u8).unwrap();
        Ok(NetBufPtr::new(raw_ptr, raw_ptr, size))
    }
}
