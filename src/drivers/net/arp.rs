//! ARP (Address Resolution Protocol) implementation

use alloc::vec::Vec;
use core::time::Duration;

/// ARP hardware type
const ARP_HW_TYPE_ETHERNET: u16 = 1;

/// ARP protocol type (IPv4)
const ARP_PROTO_TYPE_IPV4: u16 = 0x0800;

/// ARP operation codes
const ARP_OP_REQUEST: u16 = 1;
const ARP_OP_REPLY: u16 = 2;

/// ARP cache entry
#[derive(Debug, Clone, Copy)]
pub struct ArpEntry {
    pub ip: [u8; 4],
    pub mac: [u8; 6],
    pub timestamp: u64, // In milliseconds
}

/// ARP cache table
pub struct ArpCache {
    entries: Vec<ArpEntry>,
    timeout_ms: u64, // Entry timeout in milliseconds
}

impl ArpCache {
    /// Create a new ARP cache
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            timeout_ms: 60_000, // 60 seconds default timeout
        }
    }

    /// Add or update an entry in the cache
    pub fn add_or_update(&mut self, ip: [u8; 4], mac: [u8; 6], now_ms: u64) {
        // Check if entry already exists
        for entry in &mut self.entries {
            if entry.ip == ip {
                entry.mac = mac;
                entry.timestamp = now_ms;
                info!(
                    "ARP: Updated cache entry {}.{}.{}.{} -> {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    ip[0], ip[1], ip[2], ip[3],
                    mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
                );
                return;
            }
        }

        // Add new entry
        self.entries.push(ArpEntry {
            ip,
            mac,
            timestamp: now_ms,
        });
        info!(
            "ARP: Added cache entry {}.{}.{}.{} -> {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            ip[0], ip[1], ip[2], ip[3],
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        );
    }

    /// Lookup MAC address for an IP
    pub fn lookup(&self, ip: [u8; 4], now_ms: u64) -> Option<[u8; 6]> {
        for entry in &self.entries {
            if entry.ip == ip {
                // Check if entry is still valid
                if now_ms - entry.timestamp < self.timeout_ms {
                    return Some(entry.mac);
                } else {
                    info!("ARP: Cache entry for {}.{}.{}.{} expired", ip[0], ip[1], ip[2], ip[3]);
                }
            }
        }
        None
    }

    /// Clear expired entries
    pub fn cleanup(&mut self, now_ms: u64) {
        self.entries.retain(|entry| now_ms - entry.timestamp < self.timeout_ms);
    }
}

/// ARP packet builder and parser
pub struct ArpPacket;

impl ArpPacket {
    /// Build an ARP request packet
    pub fn build_request(
        sender_mac: [u8; 6],
        sender_ip: [u8; 4],
        target_ip: [u8; 4],
    ) -> [u8; 42] {
        let mut packet = [0u8; 42];

        // Ethernet header
        packet[0..6].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff]); // Broadcast
        packet[6..12].copy_from_slice(&sender_mac);
        packet[12..14].copy_from_slice(&0x0806u16.to_be_bytes()); // EtherType: ARP

        // ARP header
        packet[14..16].copy_from_slice(&ARP_HW_TYPE_ETHERNET.to_be_bytes()); // Hardware type
        packet[16..18].copy_from_slice(&ARP_PROTO_TYPE_IPV4.to_be_bytes()); // Protocol type
        packet[18] = 6; // Hardware address length
        packet[19] = 4; // Protocol address length
        packet[20..22].copy_from_slice(&ARP_OP_REQUEST.to_be_bytes()); // Operation: request

        // Sender hardware address
        packet[22..28].copy_from_slice(&sender_mac);
        // Sender protocol address
        packet[28..32].copy_from_slice(&sender_ip);
        // Target hardware address (unknown, all zeros for request)
        packet[32..38].copy_from_slice(&[0u8; 6]);
        // Target protocol address
        packet[38..42].copy_from_slice(&target_ip);

        packet
    }

    /// Build an ARP reply packet
    pub fn build_reply(
        sender_mac: [u8; 6],
        sender_ip: [u8; 4],
        target_mac: [u8; 6],
        target_ip: [u8; 4],
    ) -> [u8; 42] {
        let mut packet = [0u8; 42];

        // Ethernet header
        packet[0..6].copy_from_slice(&target_mac); // Unicast to requester
        packet[6..12].copy_from_slice(&sender_mac);
        packet[12..14].copy_from_slice(&0x0806u16.to_be_bytes()); // EtherType: ARP

        // ARP header
        packet[14..16].copy_from_slice(&ARP_HW_TYPE_ETHERNET.to_be_bytes());
        packet[16..18].copy_from_slice(&ARP_PROTO_TYPE_IPV4.to_be_bytes());
        packet[18] = 6; // Hardware address length
        packet[19] = 4; // Protocol address length
        packet[20..22].copy_from_slice(&ARP_OP_REPLY.to_be_bytes()); // Operation: reply

        // Sender hardware address
        packet[22..28].copy_from_slice(&sender_mac);
        // Sender protocol address
        packet[28..32].copy_from_slice(&sender_ip);
        // Target hardware address
        packet[32..38].copy_from_slice(&target_mac);
        // Target protocol address
        packet[38..42].copy_from_slice(&target_ip);

        packet
    }

    /// Parse an ARP packet and extract information
    /// Returns (operation, sender_mac, sender_ip, target_mac, target_ip)
    pub fn parse(data: &[u8]) -> Result<(u16, [u8; 6], [u8; 4], [u8; 6], [u8; 4]), &'static str> {
        if data.len() < 42 {
            return Err("Packet too short for ARP");
        }

        // Skip Ethernet header (14 bytes), parse ARP header
        let hw_type = u16::from_be_bytes([data[14], data[15]]);
        let proto_type = u16::from_be_bytes([data[16], data[17]]);
        let hw_len = data[18];
        let proto_len = data[19];
        let operation = u16::from_be_bytes([data[20], data[21]]);

        // Validate
        if hw_type != ARP_HW_TYPE_ETHERNET {
            return Err("Not Ethernet hardware type");
        }
        if proto_type != ARP_PROTO_TYPE_IPV4 {
            return Err("Not IPv4 protocol type");
        }
        if hw_len != 6 || proto_len != 4 {
            return Err("Invalid address lengths");
        }

        let mut sender_mac = [0u8; 6];
        let mut sender_ip = [0u8; 4];
        let mut target_mac = [0u8; 6];
        let mut target_ip = [0u8; 4];

        sender_mac.copy_from_slice(&data[22..28]);
        sender_ip.copy_from_slice(&data[28..32]);
        target_mac.copy_from_slice(&data[32..38]);
        target_ip.copy_from_slice(&data[38..42]);

        Ok((operation, sender_mac, sender_ip, target_mac, target_ip))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arp_request_build() {
        let sender_mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let sender_ip = [192, 168, 1, 100];
        let target_ip = [192, 168, 1, 1];

        let packet = ArpPacket::build_request(sender_mac, sender_ip, target_ip);

        // Verify Ethernet header
        assert_eq!(&packet[0..6], &[0xff; 6]); // Broadcast
        assert_eq!(&packet[6..12], &sender_mac);
        assert_eq!(u16::from_be_bytes([packet[12], packet[13]]), 0x0806);

        // Verify ARP operation
        assert_eq!(u16::from_be_bytes([packet[20], packet[21]]), ARP_OP_REQUEST);
    }

    #[test]
    fn test_arp_parse() {
        let sender_mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let sender_ip = [192, 168, 1, 100];
        let target_ip = [192, 168, 1, 1];

        let packet = ArpPacket::build_request(sender_mac, sender_ip, target_ip);
        let result = ArpPacket::parse(&packet).unwrap();

        assert_eq!(result.0, ARP_OP_REQUEST);
        assert_eq!(result.1, sender_mac);
        assert_eq!(result.2, sender_ip);
        assert_eq!(result.4, target_ip);
    }
}
