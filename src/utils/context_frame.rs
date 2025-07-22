use core::fmt::Formatter;

use aarch64_cpu::registers::*;
use log::warn;

/// A struct representing the AArch64 CPU context frame.
///
/// This context frame includes
/// * the general-purpose registers (GPRs),
/// * the stack pointer associated with EL0 (SP_EL0),
/// * the exception link register (ELR),
/// * the saved program status register (SPSR).
///
/// The `#[repr(C)]` attribute ensures that the struct has a C-compatible
/// memory layout, which is important when interfacing with hardware or
/// other low-level components.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct Aarch64ContextFrame {
    /// An array of 31 `u64` values representing the general-purpose registers.
    pub gpr: [u64; 31],
    /// The stack pointer associated with EL0 (SP_EL0)
    pub sp_el0: u64,
    /// The exception link register, which stores the return address after an exception.
    pub elr: u64,
    /// The saved program status register, which holds the state of the program at the time of an exception.
    pub spsr: u64,
}

/// Implementations of [`fmt::Display`] for [`Aarch64ContextFrame`].
impl core::fmt::Display for Aarch64ContextFrame {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), core::fmt::Error> {
        for i in 0..31 {
            write!(f, "x{:02}: {:016x}   ", i, self.gpr[i])?;
            if (i + 1) % 2 == 0 {
                writeln!(f)?;
            }
        }
        writeln!(f, "spsr:{:016x}", self.spsr)?;
        write!(f, "elr: {:016x}", self.elr)?;
        writeln!(f, "   sp_el0:  {:016x}", self.sp_el0)?;
        Ok(())
    }
}

impl Default for Aarch64ContextFrame {
    /// Returns the default context frame.
    ///
    /// The default state sets the SPSR to mask all exceptions and sets the mode to EL1h.
    fn default() -> Self {
        Aarch64ContextFrame {
            gpr: [0; 31],
            spsr: (SPSR_EL1::M::EL1h
                + SPSR_EL1::I::Masked
                + SPSR_EL1::F::Masked
                + SPSR_EL1::A::Masked
                + SPSR_EL1::D::Masked)
                .value,
            elr: 0,
            sp_el0: 0,
        }
    }
}

#[allow(unused)]
impl Aarch64ContextFrame {
    /// Returns the exception program counter (ELR).
    pub fn exception_pc(&self) -> usize {
        self.elr as usize
    }

    /// Sets the exception program counter (ELR).
    ///
    /// # Arguments
    ///
    /// * `pc` - The new program counter value.
    pub fn set_exception_pc(&mut self, pc: usize) {
        self.elr = pc as u64;
    }

    /// Sets the argument in register x0.
    ///
    /// # Arguments
    ///
    /// * `arg` - The argument to be passed in register x0.
    pub fn set_argument(&mut self, arg: usize) {
        self.gpr[0] = arg as u64;
    }

    /// Sets the value of a general-purpose register (GPR).
    ///
    /// # Arguments
    ///
    /// * `index` - The index of the general-purpose register (0 to 31).
    /// * `val` - The value to be set in the register.
    ///
    /// # Behavior
    /// - If `index` is between 0 and 30, the register at the specified index is set to `val`.
    /// - If `index` is 31, the operation is ignored, as it corresponds to the zero register
    ///   (`wzr` or `xzr` in AArch64), which always reads as zero and cannot be modified.
    ///
    /// # Panics
    /// Panics if the provided `index` is outside the range 0 to 31.
    pub fn set_gpr(&mut self, index: usize, val: usize) {
        match index {
            0..=30 => self.gpr[index] = val as u64,
            31 => warn!("Try to set zero register at index [{index}] as {val}"),
            _ => {
                panic!("Invalid general-purpose register index {index}")
            }
        }
    }

    /// Retrieves the value of a general-purpose register (GPR).
    ///
    /// # Arguments
    ///
    /// * `index` - The index of the general-purpose register (0 to 31).
    ///
    /// # Returns
    /// The value stored in the specified register.
    ///
    /// # Panics
    /// Panics if the provided `index` is not in the range 0 to 31.
    ///
    /// # Notes
    /// * For `index` 31, this method returns 0, as it corresponds to the zero register (`wzr` or `xzr` in AArch64).
    pub fn gpr(&self, index: usize) -> usize {
        match index {
            0..=30 => self.gpr[index] as usize,
            31 => 0,
            _ => {
                panic!("Invalid general-purpose register index {index}")
            }
        }
    }
}
