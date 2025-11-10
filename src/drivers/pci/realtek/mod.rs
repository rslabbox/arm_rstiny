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

use core::ptr::NonNull;

pub use common::{DmaBuffer, DmaBufferArray, MmioOps, RealtekCommon};
pub use device_info::{REALTEK_DEVICES, RealtekDeviceInfo, RealtekSeries};
pub use kernel_func::UseKernelFunc;
pub use rtl8139::Rtl8139Driver;
pub use rtl8169::Rtl8169Driver;

use crate::TinyError;
use crate::TinyResult;

/// The ethernet address of the NIC (MAC address).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EthernetAddress(pub [u8; 6]);

/// Operations that require a network device (NIC) driver to implement.
pub trait NetDriverOps {
    /// The ethernet address of the NIC.
    fn mac_address(&self) -> EthernetAddress;

    /// Whether can transmit packets.
    fn can_transmit(&self) -> bool;

    /// Whether can receive packets.
    fn can_receive(&self) -> bool;

    /// Size of the receive queue.
    fn rx_queue_size(&self) -> usize;

    /// Size of the transmit queue.
    fn tx_queue_size(&self) -> usize;

    /// Gives back the `rx_buf` to the receive queue for later receiving.
    ///
    /// `rx_buf` should be the same as the one returned by
    /// [`NetDriverOps::receive`].
    fn recycle_rx_buffer(&mut self, rx_buf: NetBufPtr) -> TinyResult;

    /// Poll the transmit queue and gives back the buffers for previous transmiting.
    /// returns [`TinyResult`].
    fn recycle_tx_buffers(&mut self) -> TinyResult;

    /// Transmits a packet in the buffer to the network, without blocking,
    /// returns [`TinyResult`].
    fn transmit(&mut self, tx_buf: NetBufPtr) -> TinyResult;

    /// Receives a packet from the network and store it in the [`NetBuf`],
    /// returns the buffer.
    ///
    /// Before receiving, the driver should have already populated some buffers
    /// in the receive queue by [`NetDriverOps::recycle_rx_buffer`].
    ///
    /// If currently no incomming packets, returns an error with type
    /// [`DevError::Again`].
    fn receive(&mut self) -> TinyResult<NetBufPtr>;

    /// Allocate a memory buffer of a specified size for network transmission,
    /// returns [`TinyResult`]
    fn alloc_tx_buffer(&mut self, size: usize) -> TinyResult<NetBufPtr>;
}

/// A raw buffer struct for network device.
pub struct NetBufPtr {
    // The raw pointer of the original object.
    raw_ptr: NonNull<u8>,
    // The pointer to the net buffer.
    buf_ptr: NonNull<u8>,
    len: usize,
}

impl NetBufPtr {
    /// Create a new [`NetBufPtr`].
    pub fn new(raw_ptr: NonNull<u8>, buf_ptr: NonNull<u8>, len: usize) -> Self {
        Self {
            raw_ptr,
            buf_ptr,
            len,
        }
    }

    /// Return raw pointer of the original object.
    pub fn raw_ptr<T>(&self) -> *mut T {
        self.raw_ptr.as_ptr() as *mut T
    }

    /// Return [`NetBufPtr`] buffer len.
    pub fn packet_len(&self) -> usize {
        self.len
    }

    /// Return [`NetBufPtr`] buffer as &[u8].
    pub fn packet(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.buf_ptr.as_ptr() as *const u8, self.len) }
    }

    /// Return [`NetBufPtr`] buffer as &mut [u8].
    pub fn packet_mut(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.buf_ptr.as_ptr(), self.len) }
    }
}

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
}

/// All supported device types.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum DeviceType {
    /// Block storage device (e.g., disk).
    Block,
    /// Character device (e.g., serial port).
    Char,
    /// Network device (e.g., ethernet card).
    Net,
    /// Graphic display device (e.g., GPU)
    Display,
    /// Input device (e.g., keyboard, mouse).
    Input,
}

/// Common operations that require all device drivers to implement.
pub trait BaseDriverOps: Send + Sync {
    /// The name of the device.
    fn device_name(&self) -> &str;

    /// The type of the device.
    fn device_type(&self) -> DeviceType;

    /// The IRQ number of the device.
    fn irq_number(&self) -> Option<u32> {
        None
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
    fn recycle_rx_buffer(&mut self, rx_buf: NetBufPtr) -> TinyResult {
        self.as_net_driver_mut().recycle_rx_buffer(rx_buf)
    }

    #[inline]
    fn recycle_tx_buffers(&mut self) -> TinyResult {
        self.as_net_driver_mut().recycle_tx_buffers()
    }

    #[inline]
    fn transmit(&mut self, tx_buf: NetBufPtr) -> TinyResult {
        self.as_net_driver_mut().transmit(tx_buf)
    }

    #[inline]
    fn receive(&mut self) -> TinyResult<NetBufPtr> {
        self.as_net_driver_mut().receive()
    }

    #[inline]
    fn alloc_tx_buffer(&mut self, size: usize) -> TinyResult<NetBufPtr> {
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
    ) -> TinyResult<Self> {
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
) -> TinyResult<RealtekDriverNic> {
    let device_info = get_device_info(vendor_id, device_id)
        .ok_or(TinyError::InvalidParameter("Invalid vendor or device ID"))?;

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
