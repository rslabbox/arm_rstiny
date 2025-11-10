//! RealTek device information and constants

/// RealTek controller series
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtekSeries {
    /// RTL8139 Fast Ethernet
    Rtl8139,
    /// RTL8169 Gigabit Ethernet
    Rtl8169,
    /// RTL8168 PCIe Gigabit Ethernet
    Rtl8168,
    /// RTL8111 PCIe Gigabit Ethernet
    Rtl8111,
}

/// RealTek device information
#[derive(Debug, Clone, Copy)]
pub struct RealtekDeviceInfo {
    pub vendor_id: u16,
    pub device_id: u16,
    pub name: &'static str,
    pub series: RealtekSeries,
    pub max_speed_mbps: u32,
}

/// RealTek device database
pub const REALTEK_DEVICES: &[RealtekDeviceInfo] = &[
    // RTL8139 series (Fast Ethernet)
    RealtekDeviceInfo {
        vendor_id: 0x10EC,
        device_id: 0x8139,
        name: "RTL8139 Fast Ethernet",
        series: RealtekSeries::Rtl8139,
        max_speed_mbps: 100,
    },
    RealtekDeviceInfo {
        vendor_id: 0x10EC,
        device_id: 0x8138,
        name: "RT8139 Fast Ethernet",
        series: RealtekSeries::Rtl8139,
        max_speed_mbps: 100,
    },
    RealtekDeviceInfo {
        vendor_id: 0x1113,
        device_id: 0x1211,
        name: "SMC1211TX EZCard 10/100",
        series: RealtekSeries::Rtl8139,
        max_speed_mbps: 100,
    },
    // RTL8169 series (Gigabit Ethernet)
    RealtekDeviceInfo {
        vendor_id: 0x10EC,
        device_id: 0x8169,
        name: "RTL8169 Gigabit Ethernet",
        series: RealtekSeries::Rtl8169,
        max_speed_mbps: 1000,
    },
    RealtekDeviceInfo {
        vendor_id: 0x10EC,
        device_id: 0x8167,
        name: "RTL8169/8110 Family",
        series: RealtekSeries::Rtl8169,
        max_speed_mbps: 1000,
    },
    RealtekDeviceInfo {
        vendor_id: 0x10EC,
        device_id: 0x8136,
        name: "RTL810xE PCIe Fast Ethernet",
        series: RealtekSeries::Rtl8169,
        max_speed_mbps: 100,
    },
    // RTL8168 series (PCIe Gigabit Ethernet)
    RealtekDeviceInfo {
        vendor_id: 0x10EC,
        device_id: 0x8168,
        name: "RTL8111/8168/8411 PCIe Gigabit Ethernet",
        series: RealtekSeries::Rtl8168,
        max_speed_mbps: 1000,
    },
    RealtekDeviceInfo {
        vendor_id: 0x10EC,
        device_id: 0x8161,
        name: "RTL8111/8168B PCIe Gigabit Ethernet",
        series: RealtekSeries::Rtl8168,
        max_speed_mbps: 1000,
    },
    RealtekDeviceInfo {
        vendor_id: 0x10EC,
        device_id: 0x8162,
        name: "RTL8111/8168B PCIe Gigabit Ethernet",
        series: RealtekSeries::Rtl8168,
        max_speed_mbps: 1000,
    },
    RealtekDeviceInfo {
        vendor_id: 0x10EC,
        device_id: 0x8166,
        name: "RTL8111/8168B PCIe Gigabit Ethernet",
        series: RealtekSeries::Rtl8168,
        max_speed_mbps: 1000,
    },
    // RTL8111 series (newer PCIe Gigabit)
    RealtekDeviceInfo {
        vendor_id: 0x10EC,
        device_id: 0x8176,
        name: "RTL8111/8168 PCIe Gigabit Ethernet",
        series: RealtekSeries::Rtl8111,
        max_speed_mbps: 1000,
    },
    RealtekDeviceInfo {
        vendor_id: 0x10EC,
        device_id: 0x8178,
        name: "RTL8111/8168B PCIe Gigabit Ethernet",
        series: RealtekSeries::Rtl8111,
        max_speed_mbps: 1000,
    },
    RealtekDeviceInfo {
        vendor_id: 0x10EC,
        device_id: 0x8179,
        name: "RTL8111/8168C PCIe Gigabit Ethernet",
        series: RealtekSeries::Rtl8111,
        max_speed_mbps: 1000,
    },
    // RTL8125 2.5G Ethernet
    RealtekDeviceInfo {
        vendor_id: 0x10EC,
        device_id: 0x8125,
        name: "RTL8125 2.5GbE Controller",
        series: RealtekSeries::Rtl8168,
        max_speed_mbps: 2500,
    },
];
