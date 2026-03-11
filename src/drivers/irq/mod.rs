//! Interrupt controller drivers.

pub mod gicv3;

pub use gicv3::{init, init_secondary, irqset_enable, irqset_register};

#[allow(unused_imports)]
pub use gicv3::{irq_handler, irqset_disable};

provider_core::define_provider!(
    provider: IRQ_PROVIDER,
    vendor_id: 0,
    device_id: 0,
    priority: 100,
    ops: crate::device::provider::IrqProvider {
        init,
        init_secondary,
        register: irqset_register,
        enable: irqset_enable,
        handle: irq_handler,
    }
);
