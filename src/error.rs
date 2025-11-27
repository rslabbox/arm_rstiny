//! Unified error types for RstinyOS kernel.

/// Unified error type for the entire kernel.
///
/// This enum contains all possible error variants from different subsystems.
/// Using a flat error structure simplifies error handling and propagation
/// across module boundaries in a no_std environment.
#[derive(thiserror::Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TinyError {
    // ============================================================================
    // PCIe Related Errors
    // ============================================================================
    /// PCIe ATU configuration timeout
    #[error("PCIe ATU configuration timeout")]
    PcieAtuTimeout,

    /// PCIe ATU not enabled after configuration
    #[error("PCIe ATU not enabled")]
    PcieAtuNotEnabled,

    /// Invalid PCIe configuration address
    #[error("PCIe invalid configuration address: {0:#x}")]
    PcieInvalidAddress(u64),

    // ============================================================================
    // PSCI (Power State Coordination Interface) Related Errors
    // ============================================================================
    /// PSCI operation not supported
    #[error("PSCI operation not supported")]
    PsciNotSupported,

    /// PSCI invalid parameters
    #[error("PSCI invalid parameters")]
    PsciInvalidParams,

    /// PSCI operation denied
    #[error("PSCI operation denied")]
    PsciDenied,

    /// PSCI CPU already on
    #[error("PSCI CPU already on")]
    PsciAlreadyOn,

    /// PSCI CPU on pending
    #[error("PSCI CPU on pending")]
    PsciOnPending,

    /// PSCI internal failure
    #[error("PSCI internal failure")]
    PsciInternalFailure,

    /// PSCI CPU not present
    #[error("PSCI CPU not present")]
    PsciNotPresent,

    /// PSCI CPU disabled
    #[error("PSCI CPU disabled")]
    PsciDisabled,

    /// PSCI invalid address
    #[error("PSCI invalid address")]
    PsciInvalidAddress,

    /// PSCI unknown method
    #[error("PSCI unknown method: {0}")]
    PsciUnknownMethod(&'static str),

    /// PSCI unknown error code
    #[error("PSCI unknown error code: {0}")]
    PsciUnknownCode(i32),

    // ============================================================================
    // IRQ (Interrupt Request) Related Errors
    // ============================================================================
    /// Invalid interrupt ID
    #[error("Invalid interrupt ID: {0}")]
    InvalidInterruptId(u32),

    /// IRQ handler not found for the given interrupt
    #[error("IRQ handler not found for interrupt {0}")]
    IrqHandlerNotFound(u32),

    /// Failed to acknowledge interrupt
    #[error("Failed to acknowledge interrupt")]
    IrqAcknowledgeFailed,

    /// GIC not initialized
    #[error("GIC not initialized")]
    GicNotInitialized,

    /// Invalid GIC pointer (null pointer)
    #[error("Invalid GIC pointer")]
    InvalidGicPointer,

    // ============================================================================
    // UART Related Errors
    // ============================================================================
    /// UART write operation failed
    #[error("UART write failed")]
    UartWriteFailed,

    /// UART read operation failed
    #[error("UART read failed")]
    UartReadFailed,

    /// UART IRQ handler invoked unexpectedly
    #[error("UART IRQ handler invoked for IRQ {0}")]
    UartIrqUnexpected(usize),

    // ============================================================================
    // Logger Related Errors
    // ============================================================================
    /// Logger already initialized
    #[error("Logger already initialized")]
    LoggerAlreadyInitialized,

    /// Logger initialization failed
    #[error("Logger initialization failed")]
    LoggerInitFailed,

    // ============================================================================
    // Console/Print Related Errors
    // ============================================================================
    /// Console write format failed
    #[error("Console write format failed")]
    ConsoleWriteFailed,

    // ============================================================================
    // Generic Errors
    // ============================================================================
    /// Operation timeout
    #[error("Operation timeout")]
    Timeout,

    /// Invalid parameter
    #[error("Invalid parameter: {0}")]
    InvalidParameter(&'static str),

    /// Generic operation failed
    #[error("Operation failed: {0}")]
    OperationFailed(&'static str),

    /// Thread join failed
    #[error("Thread Self join failed")]
    ThreadSelfJoinFailed,
}

/// Type alias for Result with TinyError as the error type.
///
/// This simplifies function signatures throughout the kernel.
/// Example: `fn configure() -> TinyResult<()>`
pub type TinyResult<T> = core::result::Result<T, TinyError>;
