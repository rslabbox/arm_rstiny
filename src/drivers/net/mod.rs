//! Network device drivers
use core::alloc::Layout;

use memory_addr::{PhysAddr, VirtAddr, pa};
use realtek_drivers::rtl8169::Rtl8169;

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


pub mod ping_packet;

pub fn test_rtl8125() {
    use ping_packet::*;
    
    let mmio_phy = pa!(0x9c0100000usize);
    let mmio_virt = crate::mm::phys_to_virt(mmio_phy).into();
    let mut rtl8169 = Rtl8169::new(mmio_virt, 0x8125);
    rtl8169.eth_probe();
    rtl8169.eth_start();

    info!("Testing RTL8125 Ethernet Driver");

    // MAC addresses and IP addresses
    let my_mac: [u8; 6] = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
    let remote_mac: [u8; 6] = [0x38, 0xf7, 0xcd, 0xc8, 0xd9, 0x32];
    let local_ip: [u8; 4] = [192, 168, 22, 102];
    let remote_ip: [u8; 4] = [192, 168, 22, 101];

    let mut packet: [u8; 128] = [0; 128];
    let mut seq = 1u16;

    // Statistics
    let mut total_sent = 0u32;
    let mut total_reply = 0u32;

    // Loop 1000 times sending ping packets
    for round in 0..1000 {
        info!("Sending ping packet, seq={} (round {}/1000)", seq, round + 1);
        
        let pak_len = generate_ping(&local_ip, &remote_ip, seq, &mut packet, &my_mac, &remote_mac);
        seq += 1;
        
        rtl8169.eth_send(&packet[..pak_len], pak_len);
        total_sent += 1;

        // Wait for response, max 1 second
        let mut ping_received = false;
        for _wait in 0..1000 {
            let mut recv_packet = [0u8; 2048];
            let recv_len = rtl8169.eth_recv(&mut recv_packet);

            if recv_len > 0 {
                info!("Received packet, length: {} bytes", recv_len);

                // Parse received packet
                let packet_type = parse_packet(&recv_packet[..recv_len as usize], &my_mac, &local_ip);

                match packet_type {
                    1 => {
                        // ARP request, generate response
                        let mut arp_reply: [u8; 128] = [0; 128];
                        let reply_len = process_arp_request(
                            &recv_packet[..recv_len as usize],
                            &my_mac,
                            &local_ip,
                            &mut arp_reply,
                        );
                        if reply_len > 0 {
                            info!("Sending ARP reply...");
                            rtl8169.eth_send(&arp_reply[..reply_len as usize], reply_len as usize);
                        }
                    }
                    2 => {
                        // Ping request, generate response
                        let mut ping_reply: [u8; 256] = [0; 256];
                        let reply_len = process_ping_request(
                            &recv_packet[..recv_len as usize],
                            &my_mac,
                            &local_ip,
                            &mut ping_reply,
                        );
                        if reply_len > 0 {
                            info!("Sending Ping reply...");
                            rtl8169.eth_send(&ping_reply[..reply_len as usize], reply_len as usize);
                        }
                    }
                    3 => {
                        // Received Ping reply
                        if !ping_received {
                            ping_received = true;
                            total_reply += 1;
                            info!("*** Ping reply received! ***");
                        }
                    }
                    _ => {}
                }

                info!("");
            }

            // Delay 1ms
            super::timer::busy_wait(core::time::Duration::from_millis(1));

            // If already received ping reply, break early
            if ping_received {
                break;
            }
        }

        if !ping_received {
            info!("*** Ping timeout! ***\n");
        }
    }

    // Display statistics
    info!("========================================");
    info!("Ping Statistics:");
    info!("  Total sent:     {} packets", total_sent);
    info!("  Total received: {} packets", total_reply);
    info!("========================================");
}
