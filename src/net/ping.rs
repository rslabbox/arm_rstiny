//! Simple ping test

extern crate alloc;

use core::time::Duration;

use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::socket::icmp::{self, PacketMetadata};
use smoltcp::wire::{IpAddress, IpCidr, Ipv4Address};

use crate::drivers::pci::realtek::{NetDriverOps, Rtl8169Driver};
use crate::drivers::timer::smoltcp_time;

use super::device::RealtekDevice;

/// Run a simple ping test
pub fn test_ping(driver: &mut Rtl8169Driver) {
    log::info!("[PING] Starting ping test...");

    // Get MAC address before creating device adapter
    let mac = driver.mac_address();
    let ethernet_addr = smoltcp::wire::EthernetAddress([
        mac.0[0], mac.0[1], mac.0[2], mac.0[3], mac.0[4], mac.0[5],
    ]);

    // Create device adapter
    let mut device = RealtekDevice::new(driver);

    // Create interface configuration
    let mut config = Config::new(ethernet_addr.into());
    config.random_seed = 0x12345678;

    // Create interface with fixed IP
    let mut iface = Interface::new(config, &mut device, smoltcp_time::now());
    iface.update_ip_addrs(|addrs| {
        addrs
            .push(IpCidr::new(IpAddress::v4(10, 19, 0, 107), 24))
            .unwrap();
    });

    // Create ICMP socket
    let icmp_rx_buffer = icmp::PacketBuffer::new(
        alloc::vec![PacketMetadata::EMPTY; 4],
        alloc::vec![0u8; 1024],
    );
    let icmp_tx_buffer = icmp::PacketBuffer::new(
        alloc::vec![PacketMetadata::EMPTY; 4],
        alloc::vec![0u8; 1024],
    );
    let icmp_socket = icmp::Socket::new(icmp_rx_buffer, icmp_tx_buffer);

    let mut sockets = SocketSet::new(alloc::vec![]);
    let icmp_handle = sockets.add(icmp_socket);

    // Target address
    let target_addr = Ipv4Address::new(10, 19, 0, 1);
    log::info!("[PING] Sending ping to {}", target_addr);

    // Send ICMP Echo Request
    let mut sent = false;
    let start_time = smoltcp_time::now();
    let timeout = smoltcp_time::duration_from_millis(10000);

    while smoltcp_time::now() - start_time < timeout {
        // Poll interface
        let timestamp = smoltcp_time::now();
        iface.poll(timestamp, &mut device, &mut sockets);

        // Send ping if not sent yet
        if !sent {
            let socket = sockets.get_mut::<icmp::Socket>(icmp_handle);
            if socket.can_send() {
                let data = b"Hello from rstiny_arm!";
                let mut buffer = alloc::vec![0u8; data.len()];
                buffer.copy_from_slice(data);

                socket.send_slice(&buffer, target_addr.into()).ok();
                sent = true;
                log::info!("[PING] Echo Request sent");
            } else {
                log::warn!("[PING] ICMP socket not ready to send");
            }
        }

        // Check for reply
        let socket = sockets.get_mut::<icmp::Socket>(icmp_handle);
        if socket.can_recv() {
            if let Ok((payload, addr)) = socket.recv() {
                log::info!("[PING] Got reply from {}: {} bytes", addr, payload.len());
                log::info!("[PING] Success!");
                return;
            } else {
                log::warn!("[PING] Failed to receive ICMP reply");
            }
        }

        // busy wait
        crate::drivers::timer::busy_wait(Duration::from_millis(100));
    }

    log::warn!("[PING] Timeout - no reply received");
}
