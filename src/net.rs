//! Network stack implementation using smoltcp

use crate::driver::{NetDriverOps, SmoltcpDevice};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::socket::icmp;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, Ipv4Address};

extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;

/// Network interface manager
pub struct NetworkInterface<T: NetDriverOps> {
    device: SmoltcpDevice<T>,
    iface: Interface,
    sockets: SocketSet<'static>,
    icmp_handle: Option<SocketHandle>,
}

impl<T: NetDriverOps> NetworkInterface<T> {
    /// Create a new network interface
    pub fn new(driver: T, ip_addr: Ipv4Address, netmask: u8) -> Self {
        let mac = driver.mac_address();
        let hw_addr = HardwareAddress::Ethernet(EthernetAddress([
            mac.0[0], mac.0[1], mac.0[2], mac.0[3], mac.0[4], mac.0[5],
        ]));

        let mut device = SmoltcpDevice::new(driver);

        let config = Config::new(hw_addr);
        let mut iface = Interface::new(config, &mut device, Instant::ZERO);

        // Configure IP address
        iface.update_ip_addrs(|addrs| {
            addrs
                .push(IpCidr::new(IpAddress::Ipv4(ip_addr), netmask))
                .expect("Failed to add IP address");
        });

        // Add default route
        iface
            .routes_mut()
            .add_default_ipv4_route(Ipv4Address::new(192, 168, 1, 1))
            .expect("Failed to add default route");

        let sockets = SocketSet::new(Vec::new());

        Self {
            device,
            iface,
            sockets,
            icmp_handle: None,
        }
    }

    /// Poll the network interface
    pub fn poll(&mut self, timestamp: Instant) {
        let _ = self
            .iface
            .poll(timestamp, &mut self.device, &mut self.sockets);
    }

    /// Send an ICMP ping (Echo Request)
    pub fn send_ping(
        &mut self,
        dest_addr: Ipv4Address,
        seq_no: u16,
        ident: u16,
        data: &[u8],
    ) -> Result<(), &'static str> {
        // Create ICMP socket if not exists
        if self.icmp_handle.is_none() {
            let icmp_rx_buffer =
                icmp::PacketBuffer::new(vec![icmp::PacketMetadata::EMPTY], vec![0; 256]);
            let icmp_tx_buffer =
                icmp::PacketBuffer::new(vec![icmp::PacketMetadata::EMPTY], vec![0; 256]);
            let mut icmp_socket = icmp::Socket::new(icmp_rx_buffer, icmp_tx_buffer);

            // Bind socket
            icmp_socket
                .bind(icmp::Endpoint::Ident(ident))
                .map_err(|_| "Failed to bind ICMP socket")?;

            let handle = self.sockets.add(icmp_socket);
            self.icmp_handle = Some(handle);
        }

        // Send ICMP Echo Request
        let handle = self.icmp_handle.unwrap();
        let socket = self.sockets.get_mut::<icmp::Socket>(handle);
        socket
            .send_slice(data, IpAddress::Ipv4(dest_addr))
            .map_err(|_| "Failed to send ICMP packet")?;

        info!(
            "Sent ICMP Echo Request to {}, seq={}, ident={}",
            dest_addr, seq_no, ident
        );

        // Poll to actually send the packet
        self.poll(Instant::ZERO);

        Ok(())
    }

    /// Receive and process ICMP Echo Reply
    pub fn recv_ping(&mut self, _timeout_ms: u64) -> Result<(Ipv4Address, u16, u16), &'static str> {
        let start = Instant::ZERO;

        if self.icmp_handle.is_none() {
            return Err("No ICMP socket available");
        }

        let handle = self.icmp_handle.unwrap();

        // Poll for response
        for _ in 0..100 {
            self.poll(start);

            let socket = self.sockets.get_mut::<icmp::Socket>(handle);
            if socket.can_recv() {
                let (data, addr) = socket.recv().map_err(|_| "Failed to receive ICMP packet")?;

                // Simple parsing - in real implementation you'd parse the ICMP header properly
                let IpAddress::Ipv4(ipv4_addr) = addr;
                // Assume it's an echo reply and extract seq/ident from data
                let seq_no = if data.len() >= 2 {
                    u16::from_be_bytes([data[0], data[1]])
                } else {
                    0
                };
                let ident = 0x1234; // Hardcoded for now

                info!(
                    "Received ICMP Echo Reply from {}, seq={}, ident={}",
                    ipv4_addr, seq_no, ident
                );
                return Ok((ipv4_addr, seq_no, ident));
            }

            // Simple delay
            for _ in 0..100000 {
                core::hint::spin_loop();
            }
        }

        Err("Timeout waiting for ICMP Echo Reply")
    }

    /// Get the MAC address of the interface
    pub fn mac_address(&self) -> [u8; 6] {
        let mac = self.device.driver().mac_address();
        mac.0
    }

    /// Get the IP address of the interface
    pub fn ip_address(&self) -> Option<Ipv4Address> {
        self.iface.ip_addrs().iter().find_map(|addr| {
            let IpAddress::Ipv4(ipv4) = addr.address();
            Some(ipv4)
        })
    }
}

/// Simple ping test function
pub fn ping_test<T: NetDriverOps>(mut driver: T, target_ip: Ipv4Address) {
    info!("=== Starting Ping Test ===");

    // First, let's check if we can receive any raw packets
    info!("Checking for raw incoming packets...");
    for i in 0..10 {
        match driver.receive() {
            Ok(rx_buf) => {
                let data = rx_buf.packet();
                info!("  [RAW RX {}] Received {} bytes", i, data.len());

                // Print first 64 bytes as hex
                let print_len = core::cmp::min(data.len(), 64);
                for (j, chunk) in data[..print_len].chunks(16).enumerate() {
                    info!("    {:04x}: {:02x?}", j * 16, chunk);
                }

                // Try to recycle buffer
                let _ = driver.recycle_rx_buffer(rx_buf);
            }
            Err(_) => {
                // No packet available
            }
        }

        // Small delay
        for _ in 0..10000 {
            core::hint::spin_loop();
        }
    }

    // Create network interface with IP 10.19.0.107/24
    let mut net_iface = NetworkInterface::new(driver, Ipv4Address::new(10, 19, 0, 107), 24);

    info!("Network interface initialized");
    info!(
        "  MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        net_iface.mac_address()[0],
        net_iface.mac_address()[1],
        net_iface.mac_address()[2],
        net_iface.mac_address()[3],
        net_iface.mac_address()[4],
        net_iface.mac_address()[5]
    );

    if let Some(ip) = net_iface.ip_address() {
        info!("  IP: {}", ip);
    }

    // Send ping
    info!("Pinging {}...", target_ip);

    for seq_no in 0..4 {
        let data = b"rstiny_ping_data";
        match net_iface.send_ping(target_ip, seq_no, 0x1234, data) {
            Ok(_) => {
                info!("  [{}/4] Sent ICMP Echo Request", seq_no + 1);

                // Wait for reply
                match net_iface.recv_ping(1000) {
                    Ok((addr, seq, _)) => {
                        info!("  [{}/4] ✓ Reply from {}, seq={}", seq_no + 1, addr, seq);
                    }
                    Err(e) => {
                        warn!("  [{}/4] ✗ {}", seq_no + 1, e);
                    }
                }
            }
            Err(e) => {
                error!("Failed to send ping: {}", e);
                break;
            }
        }

        // Delay between pings
        for _ in 0..1000000 {
            core::hint::spin_loop();
        }
    }

    info!("=== Ping Test Complete ===");
}
