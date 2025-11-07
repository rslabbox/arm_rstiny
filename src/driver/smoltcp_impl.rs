//! smoltcp integration for RealTek network drivers

use super::{NetBufPtr, NetDriverOps};
use smoltcp::phy::{
    Checksum, ChecksumCapabilities, Device, DeviceCapabilities, Medium, RxToken, TxToken,
};
use smoltcp::time::Instant;

/// Wrapper for integrating our driver with smoltcp
pub struct SmoltcpDevice<T: NetDriverOps> {
    driver: T,
    rx_buffer: Option<NetBufPtr>,
    medium: Medium,
}

impl<T: NetDriverOps> SmoltcpDevice<T> {
    /// Create a new smoltcp device wrapper
    pub fn new(driver: T) -> Self {
        Self {
            driver,
            rx_buffer: None,
            medium: Medium::Ethernet,
        }
    }

    /// Get reference to the underlying driver
    pub fn driver(&self) -> &T {
        &self.driver
    }

    /// Get mutable reference to the underlying driver
    pub fn driver_mut(&mut self) -> &mut T {
        &mut self.driver
    }
}

/// RX token for receiving packets
pub struct RealtekRxToken {
    buffer: NetBufPtr,
}

impl RxToken for RealtekRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        let packet = self.buffer.packet();
        f(packet)
    }
}

/// TX token for transmitting packets
pub struct RealtekTxToken<'a, T: NetDriverOps> {
    driver: &'a mut T,
}

impl<'a, T: NetDriverOps> TxToken for RealtekTxToken<'a, T> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        // Allocate TX buffer
        let mut tx_buf = self
            .driver
            .alloc_tx_buffer(len)
            .expect("Failed to allocate TX buffer");

        // Let smoltcp write the packet
        let packet = tx_buf.packet_mut();
        let result = f(packet);

        // Transmit the packet
        self.driver
            .transmit(tx_buf)
            .expect("Failed to transmit packet");

        result
    }
}

impl<T: NetDriverOps> Device for SmoltcpDevice<T> {
    type RxToken<'a>
        = RealtekRxToken
    where
        Self: 'a;
    type TxToken<'a>
        = RealtekTxToken<'a, T>
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        // Try to receive a packet
        match self.driver.receive() {
            Ok(rx_buf) => {
                let rx_token = RealtekRxToken { buffer: rx_buf };
                let tx_token = RealtekTxToken {
                    driver: &mut self.driver,
                };
                Some((rx_token, tx_token))
            }
            Err(_) => None,
        }
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        // Check if we can transmit
        if self.driver.can_transmit() {
            Some(RealtekTxToken {
                driver: &mut self.driver,
            })
        } else {
            None
        }
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = self.medium;
        caps.max_transmission_unit = 1536; // Standard Ethernet MTU + some headroom
        caps.max_burst_size = Some(1);

        // Hardware checksum offloading capabilities
        // Most RealTek NICs support checksum offloading
        caps.checksum = ChecksumCapabilities::default();
        caps.checksum.ipv4 = Checksum::Both;
        caps.checksum.tcp = Checksum::Both;
        caps.checksum.udp = Checksum::Both;
        caps.checksum.icmpv4 = Checksum::Both;

        caps
    }
}
