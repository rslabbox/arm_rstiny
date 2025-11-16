//! Simple network stack for RTL8125 testing
//!
//! This module provides basic Ethernet, IP, ICMP, and ARP handling for ping support.

use super::rtl8125::Rtl8125;
use super::arp::{ArpCache, ArpPacket};

/// Ethernet frame header
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct EthHeader {
    dst_mac: [u8; 6],
    src_mac: [u8; 6],
    eth_type: u16, // Big endian
}

/// IP header (simplified, IPv4 only)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct IpHeader {
    version_ihl: u8,   // Version (4 bits) + IHL (4 bits)
    tos: u8,           // Type of Service
    total_len: u16,    // Total Length (big endian)
    id: u16,           // Identification (big endian)
    flags_offset: u16, // Flags (3 bits) + Fragment Offset (13 bits) (big endian)
    ttl: u8,           // Time to Live
    protocol: u8,      // Protocol
    checksum: u16,     // Header Checksum (big endian)
    src_ip: [u8; 4],   // Source IP
    dst_ip: [u8; 4],   // Destination IP
}

/// ICMP header
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct IcmpHeader {
    icmp_type: u8,
    code: u8,
    checksum: u16, // Big endian
    id: u16,       // Big endian
    seq: u16,      // Big endian
}

const ETH_TYPE_IP: u16 = 0x0800;
const ETH_TYPE_ARP: u16 = 0x0806;
const IP_PROTO_ICMP: u8 = 1;
const ICMP_ECHO_REQUEST: u8 = 8;
const ICMP_ECHO_REPLY: u8 = 0;

/// Calculate IP checksum
fn ip_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;

    // Sum 16-bit words
    for i in (0..data.len()).step_by(2) {
        if i + 1 < data.len() {
            let word = ((data[i] as u32) << 8) | (data[i + 1] as u32);
            sum += word;
        } else {
            sum += (data[i] as u32) << 8;
        }
    }

    // Add carry
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    // One's complement
    !sum as u16
}

/// Simple network handler for ping support
pub struct NetStack {
    rtl: Rtl8125,
    local_ip: [u8; 4],
    arp_cache: ArpCache,
    timestamp_ms: u64, // Current time in milliseconds for ARP cache
}

impl NetStack {
    /// Create a new network stack
    pub fn new(rtl: Rtl8125, local_ip: [u8; 4]) -> Self {
        Self {
            rtl,
            local_ip,
            arp_cache: ArpCache::new(),
            timestamp_ms: 0,
        }
    }

    /// Initialize the network stack
    pub fn init(&mut self) -> Result<(), &'static str> {
        self.rtl.init()?;
        info!("NetStack: Initialized with IP {:?}", self.local_ip);
        Ok(())
    }

    /// Process incoming packets
    pub fn poll(&mut self) -> Result<(), &'static str> {
        let mut buf = [0u8; 1536];

        match self.rtl.recv(&mut buf) {
            Ok(len) => {
                self.handle_packet(&buf[..len])?;
                Ok(())
            }
            Err(_) => Ok(()), // No packet available
        }
    }

    /// Handle received packet
    fn handle_packet(&mut self, data: &[u8]) -> Result<(), &'static str> {
        if data.len() < core::mem::size_of::<EthHeader>() {
            return Err("Packet too short");
        }

        // Parse Ethernet header
        let eth_type = u16::from_be_bytes([data[12], data[13]]);

        match eth_type {
            ETH_TYPE_IP => self.handle_ip_packet(data)?,
            ETH_TYPE_ARP => self.handle_arp_packet(data)?,
            _ => {
                debug!("NetStack: Unknown ethernet type: 0x{:04x}", eth_type);
            }
        }

        Ok(())
    }

    /// Handle IP packet
    fn handle_ip_packet(&mut self, data: &[u8]) -> Result<(), &'static str> {
        let eth_hdr_len = core::mem::size_of::<EthHeader>();

        if data.len() < eth_hdr_len + core::mem::size_of::<IpHeader>() {
            return Err("IP packet too short");
        }

        let ip_data = &data[eth_hdr_len..];
        let version_ihl = ip_data[0];
        let version = version_ihl >> 4;
        let ihl = (version_ihl & 0x0F) as usize * 4;

        if version != 4 {
            return Err("Not IPv4");
        }

        let protocol = ip_data[9];
        let dst_ip = [ip_data[16], ip_data[17], ip_data[18], ip_data[19]];

        // Check if packet is for us
        if dst_ip != self.local_ip {
            return Ok(()); // Not for us
        }

        match protocol {
            IP_PROTO_ICMP => self.handle_icmp_packet(data, ihl)?,
            _ => {
                debug!("NetStack: Unsupported IP protocol: {}", protocol);
            }
        }

        Ok(())
    }

    /// Handle ARP packet
    fn handle_arp_packet(&mut self, data: &[u8]) -> Result<(), &'static str> {
        match ArpPacket::parse(data) {
            Ok((operation, sender_mac, sender_ip, _target_mac, target_ip)) => {
                // Update ARP cache with sender's info
                self.arp_cache
                    .add_or_update(sender_ip, sender_mac, self.timestamp_ms);

                if operation == 1 {
                    // ARP Request
                    info!(
                        "NetStack: ARP Request from {}.{}.{}.{} asking for {}.{}.{}.{}",
                        sender_ip[0],
                        sender_ip[1],
                        sender_ip[2],
                        sender_ip[3],
                        target_ip[0],
                        target_ip[1],
                        target_ip[2],
                        target_ip[3]
                    );

                    // Check if the request is for us
                    if target_ip == self.local_ip {
                        // Send ARP reply
                        let reply = ArpPacket::build_reply(
                            self.rtl.mac_address(),
                            self.local_ip,
                            sender_mac,
                            sender_ip,
                        );
                        info!("NetStack: Sending ARP Reply");
                        self.rtl.send(&reply)?;
                    }
                } else if operation == 2 {
                    // ARP Reply
                    info!(
                        "NetStack: ARP Reply from {}.{}.{}.{} (MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
                        sender_ip[0],
                        sender_ip[1],
                        sender_ip[2],
                        sender_ip[3],
                        sender_mac[0],
                        sender_mac[1],
                        sender_mac[2],
                        sender_mac[3],
                        sender_mac[4],
                        sender_mac[5]
                    );
                }
            }
            Err(e) => {
                debug!("NetStack: Failed to parse ARP packet: {}", e);
            }
        }

        Ok(())
    }

    /// Handle ICMP packet (ping request/reply)
    fn handle_icmp_packet(&mut self, data: &[u8], ip_hdr_len: usize) -> Result<(), &'static str> {
        let eth_hdr_len = core::mem::size_of::<EthHeader>();
        let icmp_offset = eth_hdr_len + ip_hdr_len;

        if data.len() < icmp_offset + core::mem::size_of::<IcmpHeader>() {
            return Err("ICMP packet too short");
        }

        let icmp_type = data[icmp_offset];

        if icmp_type == ICMP_ECHO_REQUEST {
            info!("NetStack: Received ICMP Echo Request (ping)");
            self.send_icmp_reply(data, ip_hdr_len)?;
        } else {
            debug!("NetStack: ICMP type: {}", icmp_type);
        }

        Ok(())
    }

    /// Send ICMP Echo Reply
    fn send_icmp_reply(&mut self, request: &[u8], ip_hdr_len: usize) -> Result<(), &'static str> {
        let eth_hdr_len = core::mem::size_of::<EthHeader>();
        let mut reply = [0u8; 1536];
        let reply_len = request.len();

        if reply_len > reply.len() {
            return Err("Reply too large");
        }

        // Copy original packet
        reply[..reply_len].copy_from_slice(&request[..reply_len]);

        // Swap MAC addresses
        let src_mac = self.rtl.mac_address();
        reply[0..6].copy_from_slice(&request[6..12]); // dst = src
        reply[6..12].copy_from_slice(&src_mac); // src = our MAC

        // Swap IP addresses
        let ip_offset = eth_hdr_len;
        let src_ip = [
            request[ip_offset + 12],
            request[ip_offset + 13],
            request[ip_offset + 14],
            request[ip_offset + 15],
        ];
        reply[ip_offset + 12..ip_offset + 16].copy_from_slice(&self.local_ip);
        reply[ip_offset + 16..ip_offset + 20].copy_from_slice(&src_ip);

        // Recalculate IP checksum
        reply[ip_offset + 10] = 0;
        reply[ip_offset + 11] = 0;
        let ip_hchecksum = ip_checksum(&reply[ip_offset..ip_offset + ip_hdr_len]);
        reply[ip_offset + 10..ip_offset + 12].copy_from_slice(&ip_hchecksum.to_be_bytes());

        // Change ICMP type to Echo Reply
        let icmp_offset = eth_hdr_len + ip_hdr_len;
        reply[icmp_offset] = ICMP_ECHO_REPLY;

        // Recalculate ICMP checksum
        reply[icmp_offset + 2] = 0;
        reply[icmp_offset + 3] = 0;
        let icmp_len = reply_len - icmp_offset;
        let icmp_checksum = ip_checksum(&reply[icmp_offset..icmp_offset + icmp_len]);
        reply[icmp_offset + 2..icmp_offset + 4].copy_from_slice(&icmp_checksum.to_be_bytes());

        // Send reply
        info!("NetStack: Sending ICMP Echo Reply");
        self.rtl.send(&reply[..reply_len])?;

        Ok(())
    }

    /// Get MAC address
    pub fn mac_address(&self) -> [u8; 6] {
        self.rtl.mac_address()
    }

    /// Send ICMP Echo Request (ping)
    pub fn send_ping(&mut self, target_ip: [u8; 4], seq: u16) -> Result<(), &'static str> {
        // First, check if we have the target's MAC address in cache
        let target_mac = match self.arp_cache.lookup(target_ip, self.timestamp_ms) {
            Some(mac) => {
                info!(
                    "NetStack: Found target MAC in cache: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
                );
                mac
            }
            None => {
                info!("NetStack: Target MAC not in cache, performing ARP request");
                // Send ARP request
                let arp_request = ArpPacket::build_request(
                    self.rtl.mac_address(),
                    self.local_ip,
                    target_ip,
                );
                self.rtl.send(&arp_request)?;

                // Wait for ARP reply (poll for up to 1 second)
                let mut target_mac = None;
                for _ in 0..100 {
                    // Wait 10ms
                    crate::drivers::timer::busy_wait(core::time::Duration::from_millis(10));
                    self.timestamp_ms += 10;

                    // Check for ARP reply
                    let mut buf = [0u8; 1536];
                    match self.rtl.recv(&mut buf) {
                        Ok(len) => {
                            // Try to parse as ARP
                            if let Ok((operation, sender_mac, sender_ip, _, _)) =
                                ArpPacket::parse(&buf[..len])
                            {
                                if operation == 2 && sender_ip == target_ip {
                                    // ARP Reply from target
                                    info!(
                                        "NetStack: Received ARP reply from {}.{}.{}.{} (MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
                                        sender_ip[0],
                                        sender_ip[1],
                                        sender_ip[2],
                                        sender_ip[3],
                                        sender_mac[0],
                                        sender_mac[1],
                                        sender_mac[2],
                                        sender_mac[3],
                                        sender_mac[4],
                                        sender_mac[5]
                                    );
                                    self.arp_cache
                                        .add_or_update(sender_ip, sender_mac, self.timestamp_ms);
                                    target_mac = Some(sender_mac);
                                    break;
                                }
                            }
                        }
                        Err(_) => continue,
                    }
                }

                match target_mac {
                    Some(mac) => mac,
                    None => {
                        return Err("ARP resolution failed: no reply");
                    }
                }
            }
        };

        // Now we have the target MAC, construct and send ICMP Echo Request
        let mut packet = [0u8; 98]; // Ethernet(14) + IP(20) + ICMP(8) + payload(56)
        let eth_hdr_len = 14;
        let ip_hdr_len = 20;
        let icmp_hdr_len = 8;
        let payload_len = 56;

        // Ethernet header
        // Destination MAC: now we use the resolved target MAC
        packet[0..6].copy_from_slice(&target_mac);
        // Source MAC: our MAC
        packet[6..12].copy_from_slice(&self.rtl.mac_address());
        // EtherType: IP
        packet[12..14].copy_from_slice(&ETH_TYPE_IP.to_be_bytes());

        // IP header
        let ip_offset = eth_hdr_len;
        packet[ip_offset] = 0x45; // Version 4, IHL 5
        packet[ip_offset + 1] = 0; // TOS
        let total_len = (ip_hdr_len + icmp_hdr_len + payload_len) as u16;
        packet[ip_offset + 2..ip_offset + 4].copy_from_slice(&total_len.to_be_bytes());
        packet[ip_offset + 4..ip_offset + 6].copy_from_slice(&0x1234u16.to_be_bytes()); // ID
        packet[ip_offset + 6..ip_offset + 8].copy_from_slice(&0x0000u16.to_be_bytes()); // Flags + Fragment
        packet[ip_offset + 8] = 64; // TTL
        packet[ip_offset + 9] = IP_PROTO_ICMP; // Protocol
        packet[ip_offset + 10] = 0; // Checksum (will calculate)
        packet[ip_offset + 11] = 0;
        packet[ip_offset + 12..ip_offset + 16].copy_from_slice(&self.local_ip); // Source IP
        packet[ip_offset + 16..ip_offset + 20].copy_from_slice(&target_ip); // Dest IP

        // Calculate IP checksum
        let ip_csum = ip_checksum(&packet[ip_offset..ip_offset + ip_hdr_len]);
        packet[ip_offset + 10..ip_offset + 12].copy_from_slice(&ip_csum.to_be_bytes());

        // ICMP header
        let icmp_offset = eth_hdr_len + ip_hdr_len;
        packet[icmp_offset] = ICMP_ECHO_REQUEST; // Type
        packet[icmp_offset + 1] = 0; // Code
        packet[icmp_offset + 2] = 0; // Checksum (will calculate)
        packet[icmp_offset + 3] = 0;
        packet[icmp_offset + 4..icmp_offset + 6].copy_from_slice(&0x1234u16.to_be_bytes()); // ID
        packet[icmp_offset + 6..icmp_offset + 8].copy_from_slice(&seq.to_be_bytes()); // Sequence

        // ICMP payload (填充数据)
        for i in 0..payload_len {
            packet[icmp_offset + icmp_hdr_len + i] = (0x20 + (i % 64)) as u8;
        }

        // Calculate ICMP checksum
        let icmp_len = icmp_hdr_len + payload_len;
        let icmp_csum = ip_checksum(&packet[icmp_offset..icmp_offset + icmp_len]);
        packet[icmp_offset + 2..icmp_offset + 4].copy_from_slice(&icmp_csum.to_be_bytes());

        // Send packet
        self.rtl.send(&packet)?;

        Ok(())
    }

    /// Check for ping reply
    pub fn check_ping_reply(&mut self, target_ip: [u8; 4], seq: u16) -> Result<bool, &'static str> {
        let mut buf = [0u8; 1536];

        match self.rtl.recv(&mut buf) {
            Ok(len) => {
                info!("NetStack: Received packet of length {}", len);

                // Check EtherType first
                if len < 14 {
                    return Ok(false);
                }

                let eth_type = u16::from_be_bytes([buf[12], buf[13]]);
                
                // Handle ARP packets (update cache)
                if eth_type == ETH_TYPE_ARP {
                    if let Ok((operation, sender_mac, sender_ip, _, _)) = ArpPacket::parse(&buf[..len]) {
                        if operation == 2 {
                            // ARP Reply
                            self.arp_cache.add_or_update(sender_ip, sender_mac, self.timestamp_ms);
                            info!(
                                "NetStack: ARP Reply cached: {}.{}.{}.{} -> {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                                sender_ip[0], sender_ip[1], sender_ip[2], sender_ip[3],
                                sender_mac[0], sender_mac[1], sender_mac[2], sender_mac[3], sender_mac[4], sender_mac[5]
                            );
                        }
                    }
                    return Ok(false); // Not a ping reply
                }
                
                // Check if it's an ICMP Echo Reply from our target
                if len < 14 + 20 + 8 {
                    warn!("NetStack: Packet too short ({} < 42)", len);
                    return Ok(false);
                }

                info!("NetStack: EtherType: 0x{:04x}", eth_type);
                if eth_type != ETH_TYPE_IP {
                    warn!("NetStack: Not IP packet");
                    return Ok(false);
                }

                let ip_offset = 14;
                let protocol = buf[ip_offset + 9];
                info!("NetStack: IP Protocol: {}", protocol);
                if protocol != IP_PROTO_ICMP {
                    warn!("NetStack: Not ICMP");
                    return Ok(false);
                }

                let src_ip = [
                    buf[ip_offset + 12],
                    buf[ip_offset + 13],
                    buf[ip_offset + 14],
                    buf[ip_offset + 15],
                ];
                info!(
                    "NetStack: Source IP: {}.{}.{}.{}",
                    src_ip[0], src_ip[1], src_ip[2], src_ip[3]
                );
                
                let dst_ip = [
                    buf[ip_offset + 16],
                    buf[ip_offset + 17],
                    buf[ip_offset + 18],
                    buf[ip_offset + 19],
                ];
                info!(
                    "NetStack: Dest IP: {}.{}.{}.{}",
                    dst_ip[0], dst_ip[1], dst_ip[2], dst_ip[3]
                );
                
                if src_ip != target_ip {
                    warn!(
                        "NetStack: Source IP mismatch (expected {}.{}.{}.{})",
                        target_ip[0], target_ip[1], target_ip[2], target_ip[3]
                    );
                    return Ok(false);
                }

                let icmp_offset = 14 + 20;
                let icmp_type = buf[icmp_offset];
                let icmp_code = buf[icmp_offset + 1];
                let icmp_seq = u16::from_be_bytes([buf[icmp_offset + 6], buf[icmp_offset + 7]]);
                
                info!(
                    "NetStack: ICMP Type: {}, Code: {}, Seq: {}",
                    icmp_type, icmp_code, icmp_seq
                );

                if icmp_type == ICMP_ECHO_REPLY && icmp_seq == seq {
                    info!("NetStack: ✓ Valid ICMP Echo Reply!");
                    return Ok(true);
                } else if icmp_type == ICMP_ECHO_REQUEST {
                    info!("NetStack: This is our own Echo Request (looped back)");
                    return Ok(false);
                } else {
                    warn!("NetStack: ICMP type or seq mismatch (expected type=0, seq={})", seq);
                    return Ok(false);
                }
            }
            Err(_) => Ok(false), // No packet available
        }
    }

    /// Update internal timestamp (for ARP cache aging)
    pub fn tick(&mut self, delta_ms: u64) {
        self.timestamp_ms += delta_ms;
        self.arp_cache.cleanup(self.timestamp_ms);
    }
}

/// Test network stack with ping support
pub fn test_ping(mmio_base: memory_addr::PhysAddr, local_ip: [u8; 4]) {
    info!("=== Testing RTL8125 Active Ping ===");
    info!(
        "Local IP: {}.{}.{}.{}",
        local_ip[0], local_ip[1], local_ip[2], local_ip[3]
    );

    let target_ip = [192, 168, 22, 101]; // 目标IP
    info!(
        "Target IP: {}.{}.{}.{}",
        target_ip[0], target_ip[1], target_ip[2], target_ip[3]
    );

    let rtl = Rtl8125::new(mmio_base);
    let mut net = NetStack::new(rtl, local_ip);

    match net.init() {
        Ok(_) => {
            info!("NetStack: Initialized successfully");
            info!(
                "NetStack: MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                net.mac_address()[0],
                net.mac_address()[1],
                net.mac_address()[2],
                net.mac_address()[3],
                net.mac_address()[4],
                net.mac_address()[5]
            );

            // 发送 10 次 ping，每次间隔约 1 秒
            for seq in 0..10 {
                info!(
                    "Sending ping to {}.{}.{}.{} (seq={})",
                    target_ip[0], target_ip[1], target_ip[2], target_ip[3], seq
                );

                match net.send_ping(target_ip, seq) {
                    Ok(_) => {
                        // 等待回复 (等待约 1 秒，期间轮询接收)
                        let mut replied = false;
                        for _ in 0..100 {
                            // 延时 10ms
                            crate::drivers::timer::busy_wait(core::time::Duration::from_millis(10));
                            net.tick(10); // Update timestamp

                            match net.check_ping_reply(target_ip, seq) {
                                Ok(true) => {
                                    info!(
                                        "✓ Ping reply received from {}.{}.{}.{} (seq={})",
                                        target_ip[0], target_ip[1], target_ip[2], target_ip[3], seq
                                    );
                                    replied = true;
                                    break;
                                }
                                Ok(false) => continue,
                                Err(e) => {
                                    warn!("Error checking reply: {}", e);
                                    break;
                                }
                            }
                        }

                        if !replied {
                            error!(
                                "✗ Ping timeout: no reply from {}.{}.{}.{} (seq={})",
                                target_ip[0], target_ip[1], target_ip[2], target_ip[3], seq
                            );
                        }
                    }
                    Err(e) => {
                        error!("✗ Failed to send ping: {}", e);
                    }
                }

                // 间隔 1 秒再发送下一个 ping
                crate::drivers::timer::busy_wait(core::time::Duration::from_secs(1));
                net.tick(1000); // Update timestamp by 1 second
            }

            info!("=== Ping test completed ===");
            info!("=== Entering response mode for 30 seconds ===");
            info!("NetStack: Now responding to ARP requests and ICMP pings from other devices");

            // 进入轮询模式，持续响应其他设备的请求
            let poll_cycles = 300; // 30 seconds with 100ms interval
            for cycle in 0..poll_cycles {
                // 延时 100ms
                crate::drivers::timer::busy_wait(core::time::Duration::from_millis(100));
                net.tick(100); // Update timestamp
                
                // Process any received packets (will auto-respond to ARP and ICMP)
                let mut buf = [0u8; 1536];
                if let Ok(len) = net.rtl.recv(&mut buf) {
                    if len > 0 {
                        let _ = net.handle_packet(&buf[..len]);
                    }
                }
                
                // 每5秒显示一次进度
                if cycle % 50 == 0 {
                    let elapsed = cycle / 10;
                    info!("NetStack: Polling... ({} seconds elapsed)", elapsed);
                }
            }

            info!("=== Response mode completed ===");
        }
        Err(e) => {
            error!("NetStack: Failed to initialize: {}", e);
        }
    }
}
