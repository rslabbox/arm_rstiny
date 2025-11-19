//! Network packet processing for ping, ARP, and other protocols

use core::ptr;

// Network protocol constants
pub const ETH_ALEN: usize = 6;
#[allow(dead_code)]
pub const ETH_HLEN: usize = 14;
pub const ETH_P_IP: u16 = 0x0800;
pub const ETH_P_ARP: u16 = 0x0806;
pub const IPPROTO_ICMP: u8 = 1;
pub const ICMP_ECHO: u8 = 8;
pub const ICMP_ECHOREPLY: u8 = 0;

pub const ARP_REQUEST: u16 = 1;
pub const ARP_REPLY: u16 = 2;
pub const ARP_HW_TYPE_ETHERNET: u16 = 1;

/// Ethernet header
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct EthHeader {
    pub dest: [u8; ETH_ALEN],
    pub src: [u8; ETH_ALEN],
    pub proto: u16,
}

/// IP header
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct IpHeader {
    pub version_ihl: u8,
    pub tos: u8,
    pub total_len: u16,
    pub id: u16,
    pub frag_off: u16,
    pub ttl: u8,
    pub protocol: u8,
    pub checksum: u16,
    pub src_addr: u32,
    pub dest_addr: u32,
}

/// ICMP header
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct IcmpHeader {
    pub type_: u8,
    pub code: u8,
    pub checksum: u16,
    pub id: u16,
    pub sequence: u16,
}

/// ARP header
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ArpHeader {
    pub hw_type: u16,
    pub proto_type: u16,
    pub hw_addr_len: u8,
    pub proto_addr_len: u8,
    pub opcode: u16,
    pub sender_mac: [u8; ETH_ALEN],
    pub sender_ip: [u8; 4],
    pub target_mac: [u8; ETH_ALEN],
    pub target_ip: [u8; 4],
}

/// Calculate IP checksum
fn ip_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;

    while i + 1 < data.len() {
        let word = ((data[i] as u32) << 8) | (data[i + 1] as u32);
        sum += word;
        i += 2;
    }

    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }

    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    let val = !sum as u16;

    (val >> 8 & 0xFF)  | ((val << 8) & 0xFF00 )
}

/// Byte order conversion helpers
#[inline]
fn htons(val: u16) -> u16 {
    val.to_be()
}

#[allow(dead_code)]
#[inline]
fn htonl(val: u32) -> u32 {
    val.to_be()
}

#[inline]
fn ntohs(val: u16) -> u16 {
    u16::from_be(val)
}

#[inline]
fn ntohl(val: u32) -> u32 {
    u32::from_be(val)
}

/// Generate ICMP Echo Request (Ping) packet
///
/// # Arguments
/// * `src_ip` - Source IP address (4 bytes)
/// * `dst_ip` - Destination IP address (4 bytes)
/// * `seq` - Sequence number
/// * `packet` - Output buffer for the packet (must be at least 128 bytes)
/// * `my_mac` - Source MAC address
/// * `remote_mac` - Destination MAC address
///
/// # Returns
/// Total packet length in bytes
pub fn generate_ping(
    src_ip: &[u8; 4],
    dst_ip: &[u8; 4],
    seq: u16,
    packet: &mut [u8],
    my_mac: &[u8; 6],
    remote_mac: &[u8; 6],
) -> usize {
    info!("=== Preparing ICMP Echo Request (Ping) ===");
    info!("  Source IP: {}.{}.{}.{}", src_ip[0], src_ip[1], src_ip[2], src_ip[3]);
    info!("  Destination IP: {}.{}.{}.{}", dst_ip[0], dst_ip[1], dst_ip[2], dst_ip[3]);
    info!("  Sequence: {}", seq);

    let mut pkt_len = 0;

    // Ethernet header
    unsafe {
        let eth = &mut *(packet.as_mut_ptr() as *mut EthHeader);
        eth.dest.copy_from_slice(remote_mac);
        eth.src.copy_from_slice(my_mac);
        eth.proto = htons(ETH_P_IP);
    }
    pkt_len += core::mem::size_of::<EthHeader>();

    // IP header
    unsafe {
        let ip = &mut *(packet.as_mut_ptr().add(pkt_len) as *mut IpHeader);
        ip.version_ihl = 0x45; // IPv4, 20-byte header
        ip.tos = 0;
        ip.total_len = htons((core::mem::size_of::<IpHeader>() + core::mem::size_of::<IcmpHeader>() + 32) as u16);
        ip.id = htons(0x1234);
        ip.frag_off = 0;
        ip.ttl = 64;
        ip.protocol = IPPROTO_ICMP;
        ip.checksum = 0;
        
        // Use write_unaligned for packed struct fields - use addr_of_mut to avoid creating reference
        let src_addr = u32::from_ne_bytes(*src_ip);
        let dest_addr = u32::from_ne_bytes(*dst_ip);
        ptr::write_unaligned(ptr::addr_of_mut!(ip.src_addr), src_addr);
        ptr::write_unaligned(ptr::addr_of_mut!(ip.dest_addr), dest_addr);

        // Calculate IP checksum
        let ip_bytes = core::slice::from_raw_parts(&*ip as *const IpHeader as *const u8, core::mem::size_of::<IpHeader>());
        ip.checksum = ip_checksum(ip_bytes);
    }
    pkt_len += core::mem::size_of::<IpHeader>();

    // ICMP header
    unsafe {
        let icmp = &mut *(packet.as_mut_ptr().add(pkt_len) as *mut IcmpHeader);
        icmp.type_ = ICMP_ECHO;
        icmp.code = 0;
        icmp.checksum = 0;
        icmp.id = htons(0x5678);
        icmp.sequence = htons(seq);
    }
    pkt_len += core::mem::size_of::<IcmpHeader>();

    // ICMP payload
    for i in 0..32 {
        packet[pkt_len + i] = i as u8;
    }
    pkt_len += 32;

    // Calculate ICMP checksum
    unsafe {
        let icmp_start = core::mem::size_of::<EthHeader>() + core::mem::size_of::<IpHeader>();
        let icmp_len = core::mem::size_of::<IcmpHeader>() + 32;
        let icmp_data = &packet[icmp_start..icmp_start + icmp_len];
        let checksum = ip_checksum(icmp_data);
        let icmp = &mut *(packet.as_mut_ptr().add(icmp_start) as *mut IcmpHeader);
        icmp.checksum = checksum;
    }

    info!("  Total packet size: {} bytes", pkt_len);
    pkt_len
}

/// Parse received network packet
///
/// # Returns
/// * 0: Not for us or no action needed
/// * 1: ARP request
/// * 2: ICMP Echo request (Ping)
/// * 3: ICMP Echo reply (Ping response)
/// * -1: Parse error
pub fn parse_packet(packet: &[u8], my_mac: &[u8; 6], my_ip: &[u8; 4]) -> i32 {
    if packet.len() < core::mem::size_of::<EthHeader>() {
        debug!("Packet too short for Ethernet header");
        return -1;
    }

    let eth = unsafe { &*(packet.as_ptr() as *const EthHeader) };

    // Check if packet is for us (broadcast or unicast to our MAC)
    let is_broadcast = eth.dest.iter().all(|&b| b == 0xFF);
    let is_for_us = is_broadcast || eth.dest == *my_mac;

    if !is_for_us {
        debug!("Packet not for us, ignoring");
        return 0;
    }

    let proto = ntohs(eth.proto);
    info!("=== Received Packet ===");
    info!("  EtherType: 0x{:04x}", proto);

    // Handle ARP packets
    if proto == ETH_P_ARP {
        if packet.len() < core::mem::size_of::<EthHeader>() + core::mem::size_of::<ArpHeader>() {
            debug!("Packet too short for ARP");
            return -1;
        }

        let arp = unsafe { &*(packet.as_ptr().add(core::mem::size_of::<EthHeader>()) as *const ArpHeader) };
        let opcode = ntohs(arp.opcode);

        info!("  ARP Opcode: {} ({})", opcode, if opcode == ARP_REQUEST { "Request" } else { "Reply" });
        info!("  Target IP: {}.{}.{}.{}", arp.target_ip[0], arp.target_ip[1], arp.target_ip[2], arp.target_ip[3]);

        if opcode == ARP_REQUEST && arp.target_ip == *my_ip {
            info!("  => ARP Request for our IP, need to reply");
            return 1;
        }
        return 0;
    }

    // Handle IP packets
    if proto == ETH_P_IP {
        if packet.len() < core::mem::size_of::<EthHeader>() + core::mem::size_of::<IpHeader>() {
            debug!("Packet too short for IP header");
            return -1;
        }

        let ip = unsafe { &*(packet.as_ptr().add(core::mem::size_of::<EthHeader>()) as *const IpHeader) };

        info!("  IP Protocol: {}", ip.protocol);

        // Check if destination IP is ours
        let dest_ip = ntohl(ip.dest_addr);
        let my_ip_val = ((my_ip[0] as u32) << 24) | ((my_ip[1] as u32) << 16) | ((my_ip[2] as u32) << 8) | (my_ip[3] as u32);
        
        if dest_ip != my_ip_val {
            debug!("IP packet not for us");
            return 0;
        }

        // Handle ICMP packets
        if ip.protocol == IPPROTO_ICMP {
            if packet.len() < core::mem::size_of::<EthHeader>() + core::mem::size_of::<IpHeader>() + core::mem::size_of::<IcmpHeader>() {
                debug!("Packet too short for ICMP");
                return -1;
            }

            let icmp = unsafe { &*(packet.as_ptr().add(core::mem::size_of::<EthHeader>() + core::mem::size_of::<IpHeader>()) as *const IcmpHeader) };

            info!("  ICMP Type: {}", icmp.type_);

            if icmp.type_ == ICMP_ECHO {
                info!("  => ICMP Echo Request (Ping), need to reply");
                return 2;
            } else if icmp.type_ == ICMP_ECHOREPLY {
                info!("  => ICMP Echo Reply (Ping response)");
                info!("  Sequence: {}", ntohs(icmp.sequence));
                return 3;
            }
        }
    }

    0
}

/// Process ARP request and generate reply
///
/// # Returns
/// Length of reply packet, or negative on error
pub fn process_arp_request(
    packet: &[u8],
    my_mac: &[u8; 6],
    my_ip: &[u8; 4],
    reply_packet: &mut [u8],
) -> isize {
    if packet.len() < core::mem::size_of::<EthHeader>() + core::mem::size_of::<ArpHeader>() {
        error!("Invalid ARP packet length");
        return -1;
    }

    let _recv_eth = unsafe { &*(packet.as_ptr() as *const EthHeader) };
    let recv_arp = unsafe { &*(packet.as_ptr().add(core::mem::size_of::<EthHeader>()) as *const ArpHeader) };

    info!("=== Generating ARP Reply ===");

    // Construct Ethernet header
    unsafe {
        let send_eth = &mut *(reply_packet.as_mut_ptr() as *mut EthHeader);
        send_eth.dest.copy_from_slice(&recv_arp.sender_mac);
        send_eth.src.copy_from_slice(my_mac);
        send_eth.proto = htons(ETH_P_ARP);
    }

    // Construct ARP reply
    unsafe {
        let send_arp = &mut *(reply_packet.as_mut_ptr().add(core::mem::size_of::<EthHeader>()) as *mut ArpHeader);
        send_arp.hw_type = htons(ARP_HW_TYPE_ETHERNET);
        send_arp.proto_type = htons(ETH_P_IP);
        send_arp.hw_addr_len = ETH_ALEN as u8;
        send_arp.proto_addr_len = 4;
        send_arp.opcode = htons(ARP_REPLY);
        send_arp.sender_mac.copy_from_slice(my_mac);
        send_arp.sender_ip.copy_from_slice(my_ip);
        send_arp.target_mac.copy_from_slice(&recv_arp.sender_mac);
        send_arp.target_ip.copy_from_slice(&recv_arp.sender_ip);
    }

    (core::mem::size_of::<EthHeader>() + core::mem::size_of::<ArpHeader>()) as isize
}

/// Process ping request and generate reply
///
/// # Returns
/// Length of reply packet, or negative on error
pub fn process_ping_request(
    packet: &[u8],
    my_mac: &[u8; 6],
    _my_ip: &[u8; 4],
    reply_packet: &mut [u8],
) -> isize {
    if packet.len() < core::mem::size_of::<EthHeader>() + core::mem::size_of::<IpHeader>() + core::mem::size_of::<IcmpHeader>() {
        error!("Invalid ICMP packet length");
        return -1;
    }

    let recv_eth = unsafe { &*(packet.as_ptr() as *const EthHeader) };
    let recv_ip = unsafe { &*(packet.as_ptr().add(core::mem::size_of::<EthHeader>()) as *const IpHeader) };
    let ip_header_len = ((recv_ip.version_ihl & 0x0F) * 4) as usize;
    let recv_icmp = unsafe { &*(packet.as_ptr().add(core::mem::size_of::<EthHeader>() + ip_header_len) as *const IcmpHeader) };

    info!("=== Generating ICMP Echo Reply (Ping Response) ===");

    // Calculate ICMP data length
    let icmp_data_len = ntohs(recv_ip.total_len) as usize - ip_header_len - core::mem::size_of::<IcmpHeader>();

    // Construct Ethernet header
    unsafe {
        let send_eth = &mut *(reply_packet.as_mut_ptr() as *mut EthHeader);
        send_eth.dest.copy_from_slice(&recv_eth.src);
        send_eth.src.copy_from_slice(my_mac);
        send_eth.proto = htons(ETH_P_IP);
    }

    // Construct IP header
    unsafe {
        let send_ip = &mut *(reply_packet.as_mut_ptr().add(core::mem::size_of::<EthHeader>()) as *mut IpHeader);
        send_ip.version_ihl = 0x45;
        send_ip.tos = 0;
        send_ip.total_len = htons((core::mem::size_of::<IpHeader>() + core::mem::size_of::<IcmpHeader>() + icmp_data_len) as u16);
        send_ip.id = recv_ip.id;
        send_ip.frag_off = 0;
        send_ip.ttl = 64;
        send_ip.protocol = IPPROTO_ICMP;
        send_ip.checksum = 0;
        send_ip.src_addr = recv_ip.dest_addr;
        send_ip.dest_addr = recv_ip.src_addr;

        let ip_bytes = core::slice::from_raw_parts(send_ip as *const IpHeader as *const u8, core::mem::size_of::<IpHeader>());
        send_ip.checksum = ip_checksum(ip_bytes);
    }

    // Construct ICMP header
    unsafe {
        let icmp_offset = core::mem::size_of::<EthHeader>() + core::mem::size_of::<IpHeader>();
        let send_icmp = &mut *(reply_packet.as_mut_ptr().add(icmp_offset) as *mut IcmpHeader);
        send_icmp.type_ = ICMP_ECHOREPLY;
        send_icmp.code = 0;
        send_icmp.checksum = 0;
        send_icmp.id = recv_icmp.id;
        send_icmp.sequence = recv_icmp.sequence;

        // Copy ICMP data
        let payload_offset = icmp_offset + core::mem::size_of::<IcmpHeader>();
        let src_data = packet.as_ptr().add(core::mem::size_of::<EthHeader>() + ip_header_len + core::mem::size_of::<IcmpHeader>());
        ptr::copy_nonoverlapping(src_data, reply_packet.as_mut_ptr().add(payload_offset), icmp_data_len);

        // Calculate ICMP checksum
        let icmp_total_len = core::mem::size_of::<IcmpHeader>() + icmp_data_len;
        let icmp_data = &reply_packet[icmp_offset..icmp_offset + icmp_total_len];
        let checksum = ip_checksum(icmp_data);
        send_icmp.checksum = checksum;
    }

    (core::mem::size_of::<EthHeader>() + core::mem::size_of::<IpHeader>() + core::mem::size_of::<IcmpHeader>() + icmp_data_len) as isize
}
