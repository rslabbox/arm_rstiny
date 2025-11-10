//! Interrupt controller drivers.

pub mod gicv3;

pub use gicv3::{init, irqset_enable, irqset_register};

#[allow(unused_imports)]
pub use gicv3::{irq_handler, irqset_disable};

