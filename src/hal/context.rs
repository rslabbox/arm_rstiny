//! Trap context for exception handling.

use core::fmt;

/// Saved registers when a trap (exception) occurs.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct TrapFrame {
    /// General-purpose registers (R0..R30).
    pub r: [u64; 31],
    /// User Stack Pointer (SP_EL0).
    pub usp: u64,
    /// Exception Link Register (ELR_EL1).
    pub elr: u64,
    /// Saved Process Status Register (SPSR_EL1).
    pub spsr: u64,
}

impl fmt::Debug for TrapFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "TrapFrame: {{")?;
        for (i, &reg) in self.r.iter().enumerate() {
            writeln!(f, "    r{i}: {reg:#x},")?;
        }
        writeln!(f, "    usp: {:#x},", self.usp)?;
        writeln!(f, "    elr: {:#x},", self.elr)?;
        writeln!(f, "    spsr: {:#x},", self.spsr)?;
        write!(f, "}}")?;
        Ok(())
    }
}
