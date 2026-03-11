//! Power management drivers.

pub mod psci;

#[allow(unused)]
pub use psci::{cpu_off, cpu_on, halt, init, system_off};

provider_core::define_provider!(
    provider: POWER_PROVIDER,
    vendor_id: 0,
    device_id: 0,
    priority: 100,
    ops: crate::device::provider::PowerProvider {
        init,
        cpu_on,
        system_off,
    }
);
