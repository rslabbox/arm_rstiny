//! Device discovery bus abstractions.

use crate::device::provider::BootProvider;
use provider_core::with_provider;

use super::model::{DeviceInfo, MAX_COMPAT_ENTRIES};

pub trait Bus {
    fn for_each_device(&self, f: impl FnMut(DeviceInfo<'_>));
}

pub struct FdtBus;
pub struct EarlyBus;

impl Bus for EarlyBus {
    fn for_each_device(&self, mut f: impl FnMut(DeviceInfo<'_>)) {
        // Synthetic early-boot devices that do not rely on FDT parsing.
        f(DeviceInfo {
            node_name: "generic-timer",
            compatible: [
                Some("arm,armv8-timer"),
                Some("arm,armv7-timer"),
                None,
                None,
                None,
                None,
                None,
                None,
            ],
            reg_base: None,
            reg_size: None,
            irq: None,
        });
    }
}

impl Bus for FdtBus {
    fn for_each_device(&self, mut f: impl FnMut(DeviceInfo<'_>)) {
        let fdt = with_provider::<BootProvider>().get_fdt().lock();

        for node in fdt.all_nodes() {
            let mut compatible = [None; MAX_COMPAT_ENTRIES];
            if let Some(comp) = node.compatible() {
                for (idx, s) in comp.all().take(MAX_COMPAT_ENTRIES).enumerate() {
                    compatible[idx] = Some(s);
                }
            }

            let reg = node.reg().next();
            let dev = DeviceInfo {
                node_name: node.name,
                compatible,
                reg_base: reg.map(|r| r.starting_address as usize),
                reg_size: reg.and_then(|r| r.size),
                irq: None,
            };
            f(dev);
        }
    }
}
