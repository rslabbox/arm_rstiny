//! Network device drivers
use core::alloc::Layout;

use memory_addr::{PhysAddr, VirtAddr};
use realtek_drivers::rtl8125::Rtl8125;

pub struct RealtekDriver;

#[crate_interface::impl_interface]
impl realtek_drivers::KernelFunc for RealtekDriver {
    fn virt_to_phys(addr: VirtAddr) -> PhysAddr {
        crate::mm::virt_to_phys(addr)
    }

    fn phys_to_virt(addr: PhysAddr) -> VirtAddr {
        crate::mm::phys_to_virt(addr.into()).into()
    }

    fn busy_wait(duration: core::time::Duration) {
        super::timer::busy_wait(duration);
    }

    fn dma_alloc_coherent(pages: usize) -> (usize, usize) {
        let allocator = crate::mm::allocator::global_allocator();
        let layout = Layout::from_size_align(pages * 4096, 4096).unwrap();
        let vaddr: *mut u8 = unsafe { allocator.alloc(layout) };
        let paddr = crate::mm::virt_to_phys(VirtAddr::from_usize(vaddr as usize)).into();
        (vaddr as usize, paddr)
    }

    fn dma_free_coherent(vaddr: usize, pages: usize) {
        let allocator = crate::mm::allocator::global_allocator();
        let layout = Layout::from_size_align(pages * 4096, 4096).unwrap();
        unsafe {
            allocator.dealloc(vaddr as *mut u8, layout);
        }
    }

    fn clean_dcache_range(addr: usize, size: usize) {
        unsafe {
            crate::hal::cpu::clean_dcache_range(addr, size);
        }
    }

    fn invalidate_dcache_range(addr: usize, size: usize) {
        unsafe {
            crate::hal::cpu::invalidate_dcache_range(addr, size);
        }
    }
}

const ETH_TYPE_ARP: u16 = 0x0806;
const ARP_HTYPE_ETHERNET: u16 = 0x0001;
const ARP_PTYPE_IPV4: u16 = 0x0800;
const ARP_OPER_REQUEST: u16 = 0x0001;

/// Send ARP request to resolve IP to MAC address
pub fn create_arp(packet: &mut [u8; 42], target_ip: [u8; 4], local_ip: [u8; 4], mac_addr: [u8; 6]) {
    // ARP packet structure:
    // Ethernet(14) + ARP(28) = 42 bytes
    // Ethernet header
    let broadcast_mac = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
    packet[0..6].copy_from_slice(&broadcast_mac); // Destination MAC: broadcast
    packet[6..12].copy_from_slice(&mac_addr); // Source MAC: our MAC
    packet[12..14].copy_from_slice(&ETH_TYPE_ARP.to_be_bytes()); // EtherType: ARP

    // ARP header
    let arp_offset = 14;
    packet[arp_offset..arp_offset + 2].copy_from_slice(&ARP_HTYPE_ETHERNET.to_be_bytes()); // Hardware type: Ethernet
    packet[arp_offset + 2..arp_offset + 4].copy_from_slice(&ARP_PTYPE_IPV4.to_be_bytes()); // Protocol type: IPv4
    packet[arp_offset + 4] = 6; // Hardware address length: 6 (MAC)
    packet[arp_offset + 5] = 4; // Protocol address length: 4 (IPv4)
    packet[arp_offset + 6..arp_offset + 8].copy_from_slice(&ARP_OPER_REQUEST.to_be_bytes()); // Operation: Request
    packet[arp_offset + 8..arp_offset + 14].copy_from_slice(&mac_addr); // Sender MAC address
    packet[arp_offset + 14..arp_offset + 18].copy_from_slice(&local_ip); // Sender IP address
    packet[arp_offset + 18..arp_offset + 24].copy_from_slice(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff]); // Target MAC address (unknown, set to 0)
    packet[arp_offset + 24..arp_offset + 28].copy_from_slice(&target_ip); // Target IP address

    info!(
        "NetStack: Sending ARP request for {}.{}.{}.{}",
        target_ip[0], target_ip[1], target_ip[2], target_ip[3]
    );
}

/// Parse and print ARP packet information
/// 
/// # Arguments
/// * `packet` - 60-byte ARP packet data
/// 
/// # ARP Packet Format (starting from Ethernet frame):
/// - 0-5: Destination MAC address (6 bytes)
/// - 6-11: Source MAC address (6 bytes)
/// - 12-13: Ethernet type 0x0806 (2 bytes)
/// - 14-15: Hardware type (2 bytes)
/// - 16-17: Protocol type (2 bytes)
/// - 18: Hardware address length (1 byte)
/// - 19: Protocol address length (1 byte)
/// - 20-21: Operation code (2 bytes)
/// - 22-27: Sender MAC address (6 bytes)
/// - 28-31: Sender IP address (4 bytes)
/// - 32-37: Target MAC address (6 bytes)
/// - 38-41: Target IP address (4 bytes)
pub fn parse_arp_packet(packet: &[u8; 60]) {
    // Parse sender MAC address (local MAC) - offset 22
    let src_mac = &packet[22..28];
    debug!(
        "Local MAC: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        src_mac[0], src_mac[1], src_mac[2], src_mac[3], src_mac[4], src_mac[5]
    );

    // Parse sender IP address (local IP) - offset 28
    let src_ip = &packet[28..32];
    debug!(
        "Local IP: {}.{}.{}.{}",
        src_ip[0], src_ip[1], src_ip[2], src_ip[3]
    );

    // Parse target MAC address - offset 32
    let dst_mac = &packet[32..38];
    debug!(
        "Target MAC: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        dst_mac[0], dst_mac[1], dst_mac[2], dst_mac[3], dst_mac[4], dst_mac[5]
    );

    // Parse target IP address - offset 38
    let dst_ip = &packet[38..42];
    debug!(
        "Target IP: {}.{}.{}.{}",
        dst_ip[0], dst_ip[1], dst_ip[2], dst_ip[3]
    );

    // Optional: print operation type
    let operation = u16::from_be_bytes([packet[20], packet[21]]);
    let op_str = match operation {
        1 => "Request",
        2 => "Reply",
        _ => "Unknown",
    };
    debug!("Operation: {}", op_str);
}

pub fn test_rtl8125() {
    let local_ip = [192, 168, 1, 60];
    let target_ip = [192, 168, 1, 8];
    info!(
        "Local IP: {}.{}.{}.{}",
        local_ip[0], local_ip[1], local_ip[2], local_ip[3]
    );

    info!(
        "Target IP: {}.{}.{}.{}",
        target_ip[0], target_ip[1], target_ip[2], target_ip[3]
    );

    let mmio_base = PhysAddr::from_usize(0x9c0100000 as usize);

    let mut rtl8125 = Rtl8125::new(mmio_base);

    rtl8125.init().unwrap();

    for _ in 0..6 {
        let mut packet = [0u8; 42];
        create_arp(&mut packet, target_ip, local_ip, rtl8125.mac_address());
        if let Err(error) = rtl8125.send(&packet) {
            log::error!("Failed to send ARP packet: {:?}", error);
            continue;
        }

        let mut buffer = [0u8; 1536];

        for _ in 0..10 {
            match rtl8125.recv(&mut buffer) {
                Ok(size) => {
                    info!("Received packet of size: {}", size);
                    parse_arp_packet(&buffer[0..60].try_into().unwrap());
                }
                Err(_) => {
                    // No packet received
                }
            }

            super::timer::busy_wait(core::time::Duration::from_millis(500));
        }
    }
}
