//! Interrupt controller drivers.

pub mod gicv3;

pub use gicv3::{init, irqset_enable, irqset_register};
