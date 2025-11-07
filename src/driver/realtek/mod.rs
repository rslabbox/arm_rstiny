//! RealTek Ethernet Driver
//!
//! This module provides driver implementations for RealTek network adapters.
//! Supports RTL8139 (Fast Ethernet) and RTL8169/RTL8168/RTL8111 (Gigabit Ethernet) series.

mod common;
mod device_info;
mod kernel_func;
mod regs;
mod rtl8139;
mod rtl8169;

use super::{BaseDriverOps, DevError, DevResult, DeviceType};
use super::{EthernetAddress, NetBufPtr, NetDriverOps};

pub use common::{DmaBuffer, DmaBufferArray, MmioOps, RealtekCommon};
pub use device_info::{REALTEK_DEVICES, RealtekDeviceInfo, RealtekSeries};
pub use kernel_func::UseKernelFunc;
pub use rtl8139::Rtl8139Driver;
pub use rtl8169::Rtl8169Driver;

/// RealTek unified driver enum
pub enum RealtekDriverNic {
    Rtl8139(Rtl8139Driver),
    Rtl8169(Rtl8169Driver),
}

// Helper methods to reduce code duplication
impl RealtekDriverNic {
    /// Execute a function with immutable driver reference
    #[inline]
    fn as_net_driver(&self) -> &dyn NetDriverOps {
        match self {
            Self::Rtl8139(driver) => driver,
            Self::Rtl8169(driver) => driver,
        }
    }

    /// Execute a function with mutable driver reference
    #[inline]
    fn as_net_driver_mut(&mut self) -> &mut dyn NetDriverOps {
        match self {
            Self::Rtl8139(driver) => driver,
            Self::Rtl8169(driver) => driver,
        }
    }

    /// Execute a function with immutable BaseDriverOps reference
    #[inline]
    fn as_base_driver(&self) -> &dyn BaseDriverOps {
        match self {
            Self::Rtl8139(driver) => driver,
            Self::Rtl8169(driver) => driver,
        }
    }

    /// Print registers for debugging (if supported)
    pub fn print_registers(&self) {
        match self {
            Self::Rtl8139(_driver) => {
                log::warn!("RTL8139 register dump not implemented");
            }
            Self::Rtl8169(driver) => {
                driver.print_registers();
            }
        }
    }
}

impl BaseDriverOps for RealtekDriverNic {
    #[inline]
    fn device_name(&self) -> &str {
        self.as_base_driver().device_name()
    }

    #[inline]
    fn device_type(&self) -> DeviceType {
        DeviceType::Net
    }
}

impl NetDriverOps for RealtekDriverNic {
    #[inline]
    fn mac_address(&self) -> EthernetAddress {
        self.as_net_driver().mac_address()
    }

    #[inline]
    fn can_transmit(&self) -> bool {
        self.as_net_driver().can_transmit()
    }

    #[inline]
    fn can_receive(&self) -> bool {
        self.as_net_driver().can_receive()
    }

    #[inline]
    fn rx_queue_size(&self) -> usize {
        self.as_net_driver().rx_queue_size()
    }

    #[inline]
    fn tx_queue_size(&self) -> usize {
        self.as_net_driver().tx_queue_size()
    }

    #[inline]
    fn recycle_rx_buffer(&mut self, rx_buf: NetBufPtr) -> DevResult {
        self.as_net_driver_mut().recycle_rx_buffer(rx_buf)
    }

    #[inline]
    fn recycle_tx_buffers(&mut self) -> DevResult {
        self.as_net_driver_mut().recycle_tx_buffers()
    }

    #[inline]
    fn transmit(&mut self, tx_buf: NetBufPtr) -> DevResult {
        self.as_net_driver_mut().transmit(tx_buf)
    }

    #[inline]
    fn receive(&mut self) -> DevResult<NetBufPtr> {
        self.as_net_driver_mut().receive()
    }

    #[inline]
    fn alloc_tx_buffer(&mut self, size: usize) -> DevResult<NetBufPtr> {
        self.as_net_driver_mut().alloc_tx_buffer(size)
    }
}

/// Device lookup helper functions
impl RealtekDriverNic {
    /// Find device info by vendor and device ID
    #[inline]
    fn find_device_info(vendor_id: u16, device_id: u16) -> Option<&'static RealtekDeviceInfo> {
        REALTEK_DEVICES
            .iter()
            .find(|info| info.vendor_id == vendor_id && info.device_id == device_id)
    }

    /// Create and initialize a driver instance based on device series
    fn create_from_info(
        device_info: &RealtekDeviceInfo,
        base_addr: usize,
        irq: u8,
    ) -> DevResult<Self> {
        let driver = match device_info.series {
            RealtekSeries::Rtl8139 => {
                let mut drv = Rtl8139Driver::new(base_addr, irq)?;
                drv.init()?;
                Self::Rtl8139(drv)
            }
            RealtekSeries::Rtl8169 | RealtekSeries::Rtl8168 | RealtekSeries::Rtl8111 => {
                let mut drv = Rtl8169Driver::new(base_addr, irq, device_info.series)?;
                drv.init()?;
                Self::Rtl8169(drv)
            }
        };
        Ok(driver)
    }
}

/// Check if PCI device is a RealTek controller
#[inline]
pub fn is_realtek_device(vendor_id: u16, device_id: u16) -> bool {
    RealtekDriverNic::find_device_info(vendor_id, device_id).is_some()
}

/// Get RealTek device information
#[inline]
pub fn get_device_info(vendor_id: u16, device_id: u16) -> Option<&'static RealtekDeviceInfo> {
    RealtekDriverNic::find_device_info(vendor_id, device_id)
}

/// Create RealTek driver from PCI device information
pub fn create_driver(
    vendor_id: u16,
    device_id: u16,
    base_addr: usize,
    irq: u8,
) -> DevResult<RealtekDriverNic> {
    let device_info = get_device_info(vendor_id, device_id).ok_or(DevError::InvalidParam)?;

    log::info!(
        "Creating RealTek driver: {} (vendor: {:#x}, device: {:#x})",
        device_info.name,
        vendor_id,
        device_id
    );

    let driver = RealtekDriverNic::create_from_info(device_info, base_addr, irq)?;

    log::info!("RealTek driver initialized successfully");

    Ok(driver)
}
