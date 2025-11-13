//! Network device adapter for smoltcp

extern crate alloc;

use smoltcp::phy::{self, Device, DeviceCapabilities, Medium};
use smoltcp::time::Instant;

use crate::drivers::pci::realtek::{NetDriverOps, Rtl8169Driver};

/// Realtek device wrapper for smoltcp
pub struct RealtekDevice<'a> {
    driver: &'a mut Rtl8169Driver,
}

impl<'a> RealtekDevice<'a> {
    pub fn new(driver: &'a mut Rtl8169Driver) -> Self {
        Self { driver }
    }
}

impl<'a> Device for RealtekDevice<'a> {
    type RxToken<'b>
        = RealtekRxToken
    where
        Self: 'b;
    type TxToken<'b>
        = RealtekTxToken<'b>
    where
        Self: 'b;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        // Try to receive a packet
        match self.driver.receive() {
            Ok(buf_ptr) => {
                let _packet_len = buf_ptr.packet_len();
                let packet_data = buf_ptr.packet().to_vec();

                // Recycle buffer
                let _ = self.driver.recycle_rx_buffer(buf_ptr);

                Some((
                    RealtekRxToken { data: packet_data },
                    RealtekTxToken {
                        driver: self.driver,
                    },
                ))
            }
            Err(_) => None,
        }
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        // Check if TX is available
        if self.driver.can_transmit() {
            Some(RealtekTxToken {
                driver: self.driver,
            })
        } else {
            None
        }
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1536;
        caps.max_burst_size = Some(1);
        caps.medium = Medium::Ethernet;
        caps
    }
}

/// RX token holding received packet data
pub struct RealtekRxToken {
    data: alloc::vec::Vec<u8>,
}

impl phy::RxToken for RealtekRxToken {
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&mut self.data)
    }
}

/// TX token for transmitting packets
pub struct RealtekTxToken<'a> {
    driver: &'a mut Rtl8169Driver,
}

impl<'a> phy::TxToken for RealtekTxToken<'a> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        // Allocate TX buffer from driver
        match self.driver.alloc_tx_buffer(len) {
            Ok(tx_buf) => {
                // Fill buffer using callback
                let result = unsafe {
                    let slice = core::slice::from_raw_parts_mut(tx_buf.raw_ptr::<u8>(), len);
                    f(slice)
                };

                // Transmit the packet
                let _ = self.driver.transmit(tx_buf);
                result
            }
            Err(_) => {
                // Fallback: use temporary buffer
                let mut buffer = alloc::vec![0u8; len];
                f(&mut buffer)
                // Note: packet will be dropped, but we return the result
            }
        }
    }
}
