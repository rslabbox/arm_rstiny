//! Power management drivers.

pub mod psci;

pub use psci::{cpu_off, cpu_on, halt, init, system_off};
