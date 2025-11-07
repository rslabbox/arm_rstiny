//! Board abstraction trait.
//!
//! This trait defines the interface that each board must implement.

/// Board-specific configuration trait.
pub trait Board {
    /// Board name.
    const NAME: &'static str;

    /// UART physical address.
    const UART_PADDR: usize;

    /// GIC Distributor base address.
    const GICD_BASE: usize;

    /// GIC Redistributor base address.
    const GICR_BASE: usize;

    /// Initialize board-specific devices.
    fn init();
}
