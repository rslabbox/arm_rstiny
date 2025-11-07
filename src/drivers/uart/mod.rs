//! UART drivers.

pub mod dw_apb;
pub mod pl011;

// Export the appropriate UART driver based on the platform
// Currently using dw_apb for OrangePi 5 Plus
pub use dw_apb::{getchar, init_early, irq_handler, putchar};
