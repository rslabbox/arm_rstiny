//! Core abstractions for device discovery and provider binding.

/// Maximum number of compatible strings captured per device.
pub const MAX_COMPAT_ENTRIES: usize = 8;

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum InitLevel {
    Early,
    Core,
    Normal,
    Late,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub struct DeviceInfo<'a> {
    pub node_name: &'a str,
    pub compatible: [Option<&'a str>; MAX_COMPAT_ENTRIES],
    pub reg_base: Option<usize>,
    pub reg_size: Option<usize>,
    pub irq: Option<u32>,
}

impl<'a> DeviceInfo<'a> {
    pub fn has_compatible(&self, target: &str) -> bool {
        self.compatible.iter().flatten().any(|c| *c == target)
    }
}
