//! Unified error types for RstinyOS kernel.
//!
//! This module uses anyhow for flexible error handling in a no_std environment.
//! All kernel subsystems use TinyResult<T> which is an alias for anyhow::Result<T>.
//!
//! ## Usage Examples
//!
//! Creating errors:
//! ```ignore
//! anyhow::bail!("Operation failed");
//! anyhow::bail!("Invalid parameter: {}", param);
//! ```
//!
//! Adding context:
//! ```ignore
//! some_operation()
//!     .context("Failed to initialize subsystem")?;
//! ```
//!
//! Ensuring conditions:
//! ```ignore
//! anyhow::ensure!(value > 0, "Value must be positive");
//! ```

/// Result type alias using anyhow::Error.
///
/// This provides flexible error handling with context and error chaining.
pub type TinyResult<T> = anyhow::Result<T>;
