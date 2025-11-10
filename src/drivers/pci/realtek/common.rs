//! Common utilities for RealTek drivers
//!
//! This module provides shared functionality to reduce code duplication
//! between RTL8139 and RTL8169 drivers.

use crate::{TinyError, TinyResult, drivers::pci::realtek::EthernetAddress};

use super::kernel_func::UseKernelFunc as KF;
use core::ptr::{read_volatile, write_volatile};

/// MMIO register access helper
pub struct MmioOps {
    base_addr: usize,
}

impl MmioOps {
    /// Create a new MMIO operations helper
    #[inline]
    pub const fn new(base_addr: usize) -> Self {
        Self { base_addr }
    }

    /// Read 8-bit register
    #[inline]
    pub fn read8(&self, offset: u16) -> u8 {
        unsafe { read_volatile((self.base_addr + offset as usize) as *const u8) }
    }

    /// Read 16-bit register
    #[inline]
    pub fn read16(&self, offset: u16) -> u16 {
        unsafe { read_volatile((self.base_addr + offset as usize) as *const u16) }
    }

    /// Read 32-bit register
    #[inline]
    pub fn read32(&self, offset: u16) -> u32 {
        unsafe { read_volatile((self.base_addr + offset as usize) as *const u32) }
    }

    /// Write 8-bit register
    #[inline]
    pub fn write8(&self, offset: u16, value: u8) {
        unsafe { write_volatile((self.base_addr + offset as usize) as *mut u8, value) }
    }

    /// Write 16-bit register
    #[inline]
    pub fn write16(&self, offset: u16, value: u16) {
        unsafe { write_volatile((self.base_addr + offset as usize) as *mut u16, value) }
    }

    /// Write 32-bit register
    #[inline]
    pub fn write32(&self, offset: u16, value: u32) {
        unsafe { write_volatile((self.base_addr + offset as usize) as *mut u32, value) }
    }

    /// Get base address
    #[inline]
    pub fn base_addr(&self) -> usize {
        self.base_addr
    }
}

/// DMA buffer management
pub struct DmaBuffer {
    vaddr: usize,
    paddr: usize,
    pages: usize,
}

impl DmaBuffer {
    /// Allocate a new DMA buffer
    pub fn alloc(size: usize) -> TinyResult<Self> {
        let pages = (size + 4095) / 4096;
        let (vaddr, paddr) = KF::dma_alloc_coherent(pages);

        if vaddr == 0 {
            return Err(TinyError::NoMemory);
        }

        Ok(Self {
            vaddr,
            paddr,
            pages,
        })
    }

    /// Get virtual address
    #[inline]
    pub fn vaddr(&self) -> usize {
        self.vaddr
    }

    /// Get physical address
    #[inline]
    pub fn paddr(&self) -> usize {
        self.paddr
    }

    /// Get number of pages
    #[inline]
    pub fn pages(&self) -> usize {
        self.pages
    }

    /// Get size in bytes
    #[inline]
    pub fn size(&self) -> usize {
        self.pages * 4096
    }
}

impl Drop for DmaBuffer {
    fn drop(&mut self) {
        if self.vaddr != 0 {
            KF::dma_free_coherent(self.vaddr, self.pages);
        }
    }
}

/// DMA buffer array for descriptor rings
pub struct DmaBufferArray<const N: usize> {
    buffers: [Option<DmaBuffer>; N],
}

impl<const N: usize> DmaBufferArray<N> {
    /// Create a new empty buffer array
    pub fn new() -> Self {
        Self {
            buffers: [const { None }; N],
        }
    }

    /// Allocate all buffers in the array
    pub fn alloc_all(&mut self, buf_size: usize) -> TinyResult {
        for i in 0..N {
            self.buffers[i] = Some(DmaBuffer::alloc(buf_size)?);
        }
        Ok(())
    }

    /// Get buffer at index
    #[inline]
    pub fn get(&self, index: usize) -> Option<&DmaBuffer> {
        self.buffers.get(index)?.as_ref()
    }

    /// Get virtual address at index
    #[inline]
    pub fn vaddr(&self, index: usize) -> usize {
        self.get(index).map(|b| b.vaddr()).unwrap_or(0)
    }

    /// Get physical address at index
    #[inline]
    pub fn paddr(&self, index: usize) -> usize {
        self.get(index).map(|b| b.paddr()).unwrap_or(0)
    }
}

impl<const N: usize> Default for DmaBufferArray<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// Common utilities for RealTek drivers
pub struct RealtekCommon;

impl RealtekCommon {
    /// Read MAC address from device
    ///
    /// # Arguments
    /// * `mmio` - MMIO operations helper
    /// * `mac_offset` - Offset of MAC address registers (usually 0x00)
    pub fn read_mac_address(mmio: &MmioOps, mac_offset: u16) -> EthernetAddress {
        let mut mac = [0u8; 6];
        for i in 0..6 {
            mac[i] = mmio.read8(mac_offset + i as u16);
        }
        EthernetAddress(mac)
    }

    /// Perform software reset with timeout
    ///
    /// # Arguments
    /// * `mmio` - MMIO operations helper
    /// * `cmd_reg` - Command register offset
    /// * `reset_bit` - Reset bit mask
    /// * `timeout_ms` - Timeout in milliseconds
    pub fn software_reset(
        mmio: &MmioOps,
        cmd_reg: u16,
        reset_bit: u8,
        timeout_ms: u32,
    ) -> TinyResult {
        // Issue reset command
        let cmd = mmio.read8(cmd_reg);
        mmio.write8(cmd_reg, cmd | reset_bit);

        // Wait for reset to complete
        let iterations = timeout_ms * 100; // 10us per iteration
        for _ in 0..iterations {
            if (mmio.read8(cmd_reg) & reset_bit) == 0 {
                return Ok(());
            }
            KF::busy_wait(core::time::Duration::from_micros(10));
        }

        log::error!("Software reset timeout");
        Err(TinyError::BadState)
    }

    /// Wait for a register bit to be set/cleared
    ///
    /// # Arguments
    /// * `mmio` - MMIO operations helper
    /// * `reg` - Register offset
    /// * `mask` - Bit mask to check
    /// * `expected` - Expected value (true = set, false = cleared)
    /// * `timeout_us` - Timeout in microseconds
    pub fn wait_for_bit(
        mmio: &MmioOps,
        reg: u16,
        mask: u32,
        expected: bool,
        timeout_us: u32,
    ) -> TinyResult {
        let iterations = timeout_us / 10;
        for _ in 0..iterations {
            let value = mmio.read32(reg);
            let is_set = (value & mask) != 0;
            if is_set == expected {
                return Ok(());
            }
            KF::busy_wait(core::time::Duration::from_micros(10));
        }

        Err(TinyError::BadState)
    }

    /// Calculate pages needed for a given size
    #[inline]
    pub fn pages_for_size(size: usize) -> usize {
        (size + 4095) / 4096
    }

    /// Pad Ethernet frame to minimum size
    #[inline]
    pub fn pad_frame_size(size: usize) -> usize {
        const MIN_ETH_FRAME_SIZE: usize = 60;
        if size < MIN_ETH_FRAME_SIZE {
            MIN_ETH_FRAME_SIZE
        } else {
            size
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pages_for_size() {
        assert_eq!(RealtekCommon::pages_for_size(0), 0);
        assert_eq!(RealtekCommon::pages_for_size(1), 1);
        assert_eq!(RealtekCommon::pages_for_size(4096), 1);
        assert_eq!(RealtekCommon::pages_for_size(4097), 2);
        assert_eq!(RealtekCommon::pages_for_size(8192), 2);
    }

    #[test]
    fn test_pad_frame_size() {
        assert_eq!(RealtekCommon::pad_frame_size(0), 60);
        assert_eq!(RealtekCommon::pad_frame_size(30), 60);
        assert_eq!(RealtekCommon::pad_frame_size(60), 60);
        assert_eq!(RealtekCommon::pad_frame_size(100), 100);
        assert_eq!(RealtekCommon::pad_frame_size(1500), 1500);
    }
}
