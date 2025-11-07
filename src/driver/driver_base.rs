//! Device driver interfaces used by [ArceOS][1]. It provides common traits and
//! types for implementing a device driver.
//!
//! You have to use this crate with the following crates for corresponding
//! device types:
//!
//! - [`axdriver_block`][2]: Common traits for block storage drivers.
//! - [`axdriver_display`][3]: Common traits and types for graphics display drivers.
//! - [`axdriver_net`][4]: Common traits and types for network (NIC) drivers.
//!
//! [1]: https://github.com/arceos-org/arceos
//! [2]: ../axdriver_block/index.html
//! [3]: ../axdriver_display/index.html
//! [4]: ../axdriver_net/index.html

pub use super::{NetBuf, NetBufBox, NetBufPool};
use core::ptr::NonNull;

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

/// The error type for device operation failures.
#[derive(Debug)]
pub enum DevError {
    /// An entity already exists.
    AlreadyExists,
    /// Try again, for non-blocking APIs.
    Again,
    /// Bad internal state.
    BadState,
    /// Invalid parameter/argument.
    InvalidParam,
    /// Input/output error.
    Io,
    /// Not enough space/cannot allocate memory (DMA).
    NoMemory,
    /// Device or resource is busy.
    ResourceBusy,
    /// This operation is unsupported or unimplemented.
    Unsupported,
}

impl core::fmt::Display for DevError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DevError::AlreadyExists => write!(f, "Entity already exists"),
            DevError::Again => write!(f, "Try again"),
            DevError::BadState => write!(f, "Bad state"),
            DevError::InvalidParam => write!(f, "Invalid parameter"),
            DevError::Io => write!(f, "Input/output error"),
            DevError::NoMemory => write!(f, "Not enough memory"),
            DevError::ResourceBusy => write!(f, "Resource is busy"),
            DevError::Unsupported => write!(f, "Unsupported operation"),
        }
    }
}

/// A specialized `Result` type for device operations.
pub type DevResult<T = ()> = Result<T, DevError>;

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

/// The ethernet address of the NIC (MAC address).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EthernetAddress(pub [u8; 6]);

/// Operations that require a network device (NIC) driver to implement.
pub trait NetDriverOps: BaseDriverOps {
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
    fn recycle_rx_buffer(&mut self, rx_buf: NetBufPtr) -> DevResult;

    /// Poll the transmit queue and gives back the buffers for previous transmiting.
    /// returns [`DevResult`].
    fn recycle_tx_buffers(&mut self) -> DevResult;

    /// Transmits a packet in the buffer to the network, without blocking,
    /// returns [`DevResult`].
    fn transmit(&mut self, tx_buf: NetBufPtr) -> DevResult;

    /// Receives a packet from the network and store it in the [`NetBuf`],
    /// returns the buffer.
    ///
    /// Before receiving, the driver should have already populated some buffers
    /// in the receive queue by [`NetDriverOps::recycle_rx_buffer`].
    ///
    /// If currently no incomming packets, returns an error with type
    /// [`DevError::Again`].
    fn receive(&mut self) -> DevResult<NetBufPtr>;

    /// Allocate a memory buffer of a specified size for network transmission,
    /// returns [`DevResult`]
    fn alloc_tx_buffer(&mut self, size: usize) -> DevResult<NetBufPtr>;
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
