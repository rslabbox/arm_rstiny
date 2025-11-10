//! RealTek register definitions
#![allow(dead_code)]

/// RTL8139 register offsets
pub mod rtl8139 {
    pub const IDR0: u16 = 0x00; // MAC address
    pub const IDR4: u16 = 0x04; // MAC address (continued)
    pub const TSD0: u16 = 0x10; // Transmit Status Descriptor 0
    pub const TSD1: u16 = 0x14; // Transmit Status Descriptor 1
    pub const TSD2: u16 = 0x18; // Transmit Status Descriptor 2
    pub const TSD3: u16 = 0x1C; // Transmit Status Descriptor 3
    pub const TSAD0: u16 = 0x20; // Transmit Start Address Descriptor 0
    pub const TSAD1: u16 = 0x24; // Transmit Start Address Descriptor 1
    pub const TSAD2: u16 = 0x28; // Transmit Start Address Descriptor 2
    pub const TSAD3: u16 = 0x2C; // Transmit Start Address Descriptor 3
    pub const RBSTART: u16 = 0x30; // Receive Buffer Start Address
    pub const CR: u16 = 0x37; // Command Register
    pub const CAPR: u16 = 0x38; // Current Address of Packet Read
    pub const CBR: u16 = 0x3A; // Current Buffer Address
    pub const IMR: u16 = 0x3C; // Interrupt Mask Register
    pub const ISR: u16 = 0x3E; // Interrupt Status Register
    pub const TCR: u16 = 0x40; // Transmit Configuration Register
    pub const RCR: u16 = 0x44; // Receive Configuration Register
    pub const TCTR: u16 = 0x48; // Timer Count Register
    pub const MPC: u16 = 0x4C; // Missed Packet Counter
    pub const CONFIG1: u16 = 0x52; // Configuration Register 1
    pub const CONFIG4: u16 = 0x5A; // Configuration Register 4
    pub const MULINT: u16 = 0x5C; // Multiple Interrupt Select

    // Command Register bits
    pub const CR_RST: u8 = 1 << 4; // Reset
    pub const CR_RE: u8 = 1 << 3; // Receiver Enable
    pub const CR_TE: u8 = 1 << 2; // Transmitter Enable
    pub const CR_BUFE: u8 = 1 << 0; // Rx Buffer Empty

    // Interrupt Status/Mask Register bits
    pub const INT_ROK: u16 = 1 << 0; // Receive OK
    pub const INT_RER: u16 = 1 << 1; // Receive Error
    pub const INT_TOK: u16 = 1 << 2; // Transmit OK
    pub const INT_TER: u16 = 1 << 3; // Transmit Error
    pub const INT_RXOVW: u16 = 1 << 4; // Rx Buffer Overflow
    pub const INT_PUN: u16 = 1 << 5; // Packet Underrun / Link Change
    pub const INT_FOVW: u16 = 1 << 6; // Rx FIFO Overflow
    pub const INT_LENCHG: u16 = 1 << 13; // Cable Length Change
    pub const INT_TIMEOUT: u16 = 1 << 14; // Time Out
    pub const INT_SERR: u16 = 1 << 15; // System Error

    // Transmit Status Register bits
    pub const TSD_OWN: u32 = 1 << 13; // DMA operation completed
    pub const TSD_TUN: u32 = 1 << 14; // Transmit FIFO underrun
    pub const TSD_TOK: u32 = 1 << 15; // Transmit OK
    pub const TSD_CDH: u32 = 1 << 28; // CD Heart Beat
    pub const TSD_OWC: u32 = 1 << 29; // Out of Window Collision
    pub const TSD_TABT: u32 = 1 << 30; // Transmit Abort
    pub const TSD_CRS: u32 = 1 << 31; // Carrier Sense Lost

    // Receive Configuration Register bits
    pub const RCR_AAP: u32 = 1 << 0; // Accept All Packets
    pub const RCR_APM: u32 = 1 << 1; // Accept Physical Match
    pub const RCR_AM: u32 = 1 << 2; // Accept Multicast
    pub const RCR_AB: u32 = 1 << 3; // Accept Broadcast
    pub const RCR_AR: u32 = 1 << 4; // Accept Runt
    pub const RCR_AER: u32 = 1 << 5; // Accept Error
    pub const RCR_WRAP: u32 = 1 << 7; // Wrap
    pub const RCR_MXDMA_SHIFT: u32 = 8; // Max DMA Burst Size shift
    pub const RCR_RBLEN_SHIFT: u32 = 11; // RX Buffer Length shift
    pub const RCR_RXFTH_SHIFT: u32 = 13; // RX FIFO Threshold shift

    // Transmit Configuration Register bits
    pub const TCR_MXDMA_SHIFT: u32 = 8; // Max DMA Burst Size shift
    pub const TCR_IFG_SHIFT: u32 = 24; // Inter-frame Gap shift

    // Receive Status bits (in buffer header)
    pub const RX_ROK: u16 = 1 << 0; // Receive OK
    pub const RX_FAE: u16 = 1 << 1; // Frame Alignment Error
    pub const RX_CRC: u16 = 1 << 2; // CRC Error
    pub const RX_LONG: u16 = 1 << 3; // Long Packet
    pub const RX_RUNT: u16 = 1 << 4; // Runt Packet
    pub const RX_ISE: u16 = 1 << 5; // Invalid Symbol Error
    pub const RX_BAR: u16 = 1 << 13; // Broadcast Address Received
    pub const RX_PAM: u16 = 1 << 14; // Physical Address Matched
    pub const RX_MAR: u16 = 1 << 15; // Multicast Address Received
}

/// RTL8169/8168/8111 register offsets
pub mod rtl8169 {
    // Re-export MAC address registers for consistency
    pub const IDR0: u16 = 0x00; // MAC address (same as MAC0)
    pub const MAC0: u16 = 0x00; // MAC address
    pub const MAC4: u16 = 0x04; // MAC address (continued)
    pub const MAR0: u16 = 0x08; // Multicast filter
    pub const MAR4: u16 = 0x0C; // Multicast filter (continued)
    pub const DTCCR: u16 = 0x10; // Dump Tally Counter Command
    pub const TNPDS_LO: u16 = 0x20; // Transmit Normal Priority Descriptors (low)
    pub const TNPDS_HI: u16 = 0x24; // Transmit Normal Priority Descriptors (high)
    pub const THPDS_LO: u16 = 0x28; // Transmit High Priority Descriptors (low)
    pub const THPDS_HI: u16 = 0x2C; // Transmit High Priority Descriptors (high)
    pub const CMD: u16 = 0x37; // Command Register
    pub const TPPOLL: u16 = 0x38; // Transmit Priority Polling
    pub const IMR: u16 = 0x3C; // Interrupt Mask Register
    pub const ISR: u16 = 0x3E; // Interrupt Status Register
    pub const TCR: u16 = 0x40; // Transmit Configuration Register
    pub const RCR: u16 = 0x44; // Receive Configuration Register
    pub const TCTR: u16 = 0x48; // Timer Count Register
    pub const MPC: u16 = 0x4C; // Missed Packet Counter
    pub const CFG_9346: u16 = 0x50; // 93C46 Command Register
    pub const CONFIG0: u16 = 0x51; // Configuration Register 0
    pub const CONFIG1: u16 = 0x52; // Configuration Register 1
    pub const CONFIG2: u16 = 0x53; // Configuration Register 2
    pub const CONFIG3: u16 = 0x54; // Configuration Register 3
    pub const CONFIG4: u16 = 0x55; // Configuration Register 4
    pub const CONFIG5: u16 = 0x56; // Configuration Register 5
    pub const TIMERINT: u16 = 0x58; // Timer Interrupt
    pub const MULINT: u16 = 0x5C; // Multiple Interrupt Select
    pub const PHYAR: u16 = 0x60; // PHY Access Register
    pub const TBICSR: u16 = 0x64; // TBI Control and Status Register
    pub const PHYSTATUS: u16 = 0x6C; // PHY Status Register
    pub const RMS: u16 = 0xDA; // Rx Max Size
    pub const CPCMD: u16 = 0xE0; // C+ Command Register
    pub const RDSAR_LO: u16 = 0xE4; // Receive Descriptor Start Address (low)
    pub const RDSAR_HI: u16 = 0xE8; // Receive Descriptor Start Address (high)
    pub const ETTHR: u16 = 0xEC; // Early Transmit Threshold

    // Command Register bits
    pub const CMD_RST: u8 = 1 << 4; // Reset
    pub const CMD_RE: u8 = 1 << 3; // Receiver Enable
    pub const CMD_TE: u8 = 1 << 2; // Transmitter Enable

    // Transmit Priority Polling bits
    pub const TPPOLL_NPQ: u8 = 1 << 6; // Normal Priority Queue polling
    pub const TPPOLL_HPQ: u8 = 1 << 7; // High Priority Queue polling

    // Interrupt bits
    pub const INT_ROK: u16 = 1 << 0; // Receive OK
    pub const INT_RER: u16 = 1 << 1; // Receive Error
    pub const INT_TOK: u16 = 1 << 2; // Transmit OK
    pub const INT_TER: u16 = 1 << 3; // Transmit Error
    pub const INT_RDU: u16 = 1 << 4; // Rx Descriptor Unavailable
    pub const INT_LINKCHG: u16 = 1 << 5; // Link Change
    pub const INT_FOVW: u16 = 1 << 6; // Rx FIFO Overflow
    pub const INT_TDU: u16 = 1 << 7; // Tx Descriptor Unavailable
    pub const INT_SWINT: u16 = 1 << 8; // Software Interrupt
    pub const INT_TIMEOUT: u16 = 1 << 14; // Time Out
    pub const INT_SERR: u16 = 1 << 15; // System Error

    // 93C46 Command Register
    pub const CFG_9346_LOCK: u8 = 0x00; // Lock configuration registers
    pub const CFG_9346_UNLOCK: u8 = 0xC0; // Unlock configuration registers

    // Receive Configuration Register bits
    pub const RCR_AAP: u32 = 1 << 0; // Accept All Packets
    pub const RCR_APM: u32 = 1 << 1; // Accept Physical Match
    pub const RCR_AM: u32 = 1 << 2; // Accept Multicast
    pub const RCR_AB: u32 = 1 << 3; // Accept Broadcast
    pub const RCR_AR: u32 = 1 << 4; // Accept Runt
    pub const RCR_AER: u32 = 1 << 5; // Accept Error
    pub const RCR_WRAP: u32 = 1 << 7; // Wrap (for RTL8139 compatibility)
    pub const RCR_MXDMA_UNLIMITED: u32 = 7 << 8; // Max DMA Burst Size (unlimited)
    pub const RCR_RXFTH_NONE: u32 = 7 << 13; // Rx FIFO Threshold (no threshold)
    pub const RCR_RXFTH_64: u32 = 2 << 13; // Rx FIFO Threshold (64 bytes)
    pub const RCR_MERINT: u32 = 1 << 24; // Multiple Early Interrupt

    // Transmit Configuration Register bits
    pub const TCR_MXDMA_UNLIMITED: u32 = 7 << 8; // Max DMA Burst Size (unlimited)
    pub const TCR_IFG_NORMAL: u32 = 3 << 24; // Inter-frame Gap (normal)
    pub const TCR_LOOPBACK: u32 = 3 << 17; // Loopback mode
}

/// Descriptor format for RTL8169/8168/8111
pub mod descriptor {
    // Common descriptor bits
    pub const DESC_OWN: u32 = 1 << 31; // Ownership (1 = NIC, 0 = CPU)
    pub const DESC_EOR: u32 = 1 << 30; // End of Ring
    pub const DESC_FS: u32 = 1 << 29; // First Segment
    pub const DESC_LS: u32 = 1 << 28; // Last Segment

    // RX Descriptor opts1 bits (aliases for common use)
    pub const RX_OWN: u32 = DESC_OWN;
    pub const RX_EOR: u32 = DESC_EOR;
    pub const RX_FS: u32 = DESC_FS;
    pub const RX_LS: u32 = DESC_LS;
    pub const RX_MAR: u32 = 1 << 26; // Multicast Address Received
    pub const RX_PAM: u32 = 1 << 25; // Physical Address Matched
    pub const RX_BAR: u32 = 1 << 24; // Broadcast Address Received
    pub const RX_BOVF: u32 = 1 << 23; // Buffer Overflow
    pub const RX_FOVF: u32 = 1 << 22; // FIFO Overflow
    pub const RX_RWT: u32 = 1 << 21; // Receive Watchdog Timer Expired
    pub const RX_RES: u32 = 1 << 20; // Receive Error Summary
    pub const RX_RUNT: u32 = 1 << 19; // Runt Packet
    pub const RX_CRC: u32 = 1 << 18; // CRC Error
    pub const RX_IPF: u32 = 1 << 16; // IP Checksum Failure
    pub const RX_UDPF: u32 = 1 << 15; // UDP Checksum Failure
    pub const RX_TCPF: u32 = 1 << 14; // TCP Checksum Failure
    pub const RX_LEN_MASK: u32 = 0x3FFF; // Frame Length

    // Aliases for error checking (DESC_RX_xxx)
    pub const DESC_RX_RES: u32 = RX_RES;
    pub const DESC_RX_RWMA: u32 = RX_MAR;  // Watchdog timeout (use MAR as placeholder)
    pub const DESC_RX_RWT: u32 = RX_RWT;
    pub const DESC_RX_RUNT: u32 = RX_RUNT;
    pub const DESC_RX_LONG: u32 = 1 << 17; // Long Packet

    // TX Descriptor opts1 bits
    pub const TX_OWN: u32 = 1 << 31; // Ownership (1 = NIC, 0 = CPU)
    pub const TX_EOR: u32 = 1 << 30; // End of Ring
    pub const TX_FS: u32 = 1 << 29; // First Segment
    pub const TX_LS: u32 = 1 << 28; // Last Segment
    pub const TX_LGSEN: u32 = 1 << 27; // Large Send Enable
    pub const TX_IPCS: u32 = 1 << 18; // IP Checksum Offload
    pub const TX_UDPCS: u32 = 1 << 17; // UDP Checksum Offload
    pub const TX_TCPCS: u32 = 1 << 16; // TCP Checksum Offload
    pub const TX_LEN_MASK: u32 = 0xFFFF; // Frame Length
}
