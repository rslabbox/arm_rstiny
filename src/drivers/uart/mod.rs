//! UART drivers.

#[cfg(feature = "opi5p")]
pub mod dw_apb;
#[cfg(feature = "qemu")]
pub mod pl011;

// Export the appropriate UART driver based on the platform
// Currently using dw_apb for OrangePi 5 Plus
#[allow(unused)]
#[cfg(feature = "opi5p")]
pub use dw_apb::{getchar, init_early, irq_handler, putchar, puts};

#[allow(unused)]
#[cfg(feature = "qemu")]
pub use pl011::{getchar, init_early, putchar, puts};

crate::define_provider!(
    provider: UART_PROVIDER,
    vendor_id: 0,
    device_id: 0,
    priority: 100,
    ops: crate::device::provider::UartProvider {
        init_early,
        puts,
        putchar,
        getchar,
    }
);