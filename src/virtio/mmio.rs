//! VirtIO MMIO Operations
//!
//! This module provides safe abstractions for VirtIO MMIO register access
//! with proper alignment checking and error handling.

use crate::virtio::error::{VirtioError, VirtioResult};
use log::error;

/// Check if an address is properly aligned for 32-bit access
fn check_alignment(base_addr: usize, offset: usize) -> VirtioResult<usize> {
    let addr = base_addr + offset;

    // Check if the address is 32-bit (4-byte) aligned
    if addr % 4 != 0 {
        error!(
            "MMIO address 0x{:x} (base: 0x{:x}, offset: 0x{:x}) is not 32-bit aligned",
            addr, base_addr, offset
        );
        return Err(VirtioError::InvalidAddress);
    }

    Ok(addr)
}

/// Read a 32-bit value from MMIO with alignment checking
pub fn read_mmio_u32(base_addr: usize, offset: usize) -> u32 {
    match check_alignment(base_addr, offset) {
        Ok(addr) => {
            // Perform the volatile read
            unsafe { core::ptr::read_volatile(addr as *const u32) }
        }
        Err(_) => {
            // Return 0 on alignment error (could also panic or return Result)
            error!("Failed to read from misaligned MMIO address");
            0
        }
    }
}

/// Write a 32-bit value to MMIO with alignment checking
pub fn write_mmio_u32(base_addr: usize, offset: usize, value: u32) {
    match check_alignment(base_addr, offset) {
        Ok(addr) => {
            // Perform the volatile write
            unsafe { core::ptr::write_volatile(addr as *mut u32, value) }
        }
        Err(_) => {
            error!("Failed to write to misaligned MMIO address");
            // Could also panic here depending on requirements
        }
    }
}

/// Read a 32-bit value from MMIO with Result return type
pub fn try_read_mmio_u32(base_addr: usize, offset: usize) -> VirtioResult<u32> {
    let addr = check_alignment(base_addr, offset)?;
    Ok(unsafe { core::ptr::read_volatile(addr as *const u32) })
}

/// Write a 32-bit value to MMIO with Result return type
pub fn try_write_mmio_u32(base_addr: usize, offset: usize, value: u32) -> VirtioResult<()> {
    let addr = check_alignment(base_addr, offset)?;
    unsafe { core::ptr::write_volatile(addr as *mut u32, value) }
    Ok(())
}

/// Read a 16-bit value from MMIO with alignment checking
pub fn read_mmio_u16(base_addr: usize, offset: usize) -> u16 {
    let addr = base_addr + offset;

    // Check if the address is 16-bit (2-byte) aligned
    if addr % 2 != 0 {
        error!(
            "MMIO address 0x{:x} (base: 0x{:x}, offset: 0x{:x}) is not 16-bit aligned",
            addr, base_addr, offset
        );
        return 0;
    }

    unsafe { core::ptr::read_volatile(addr as *const u16) }
}

/// Write a 16-bit value to MMIO with alignment checking
pub fn write_mmio_u16(base_addr: usize, offset: usize, value: u16) {
    let addr = base_addr + offset;

    // Check if the address is 16-bit (2-byte) aligned
    if addr % 2 != 0 {
        error!(
            "MMIO address 0x{:x} (base: 0x{:x}, offset: 0x{:x}) is not 16-bit aligned",
            addr, base_addr, offset
        );
        return;
    }

    unsafe { core::ptr::write_volatile(addr as *mut u16, value) }
}

/// Read an 8-bit value from MMIO (no alignment check needed)
pub fn read_mmio_u8(base_addr: usize, offset: usize) -> u8 {
    let addr = base_addr + offset;
    unsafe { core::ptr::read_volatile(addr as *const u8) }
}

/// Write an 8-bit value to MMIO (no alignment check needed)
pub fn write_mmio_u8(base_addr: usize, offset: usize, value: u8) {
    let addr = base_addr + offset;
    unsafe { core::ptr::write_volatile(addr as *mut u8, value) }
}

/// Read a 64-bit value from MMIO with alignment checking
/// This reads two consecutive 32-bit values to form a 64-bit value
pub fn read_mmio_u64(base_addr: usize, offset: usize) -> u64 {
    // Read low 32 bits
    let low = read_mmio_u32(base_addr, offset);
    // Read high 32 bits
    let high = read_mmio_u32(base_addr, offset + 4);

    // Combine into 64-bit value (little-endian)
    (high as u64) << 32 | (low as u64)
}

/// Write a 64-bit value to MMIO with alignment checking
/// This writes two consecutive 32-bit values
pub fn write_mmio_u64(base_addr: usize, offset: usize, value: u64) {
    // Write low 32 bits
    write_mmio_u32(base_addr, offset, value as u32);
    // Write high 32 bits
    write_mmio_u32(base_addr, offset + 4, (value >> 32) as u32);
}
