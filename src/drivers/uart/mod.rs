//! UART drivers.

#[cfg(feature = "opi5p")]
pub mod dw_apb;
#[cfg(feature = "qemu")]
pub mod pl011;

// Export the appropriate UART driver based on the platform
// Currently using dw_apb for OrangePi 5 Plus
#[allow(unused)]
#[cfg(feature = "opi5p")]
pub use dw_apb::{getchar, init_early, irq_handler, putchar};

#[allow(unused)]
#[cfg(feature = "qemu")]
pub use pl011::{getchar, init_early, irq_handler, putchar};
