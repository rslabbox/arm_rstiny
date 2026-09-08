//! PL011 polling driver using the register layout style of arm_pl011.
use core::ptr::NonNull;
use tock_registers::{
    interfaces::{Readable, Writeable},
    register_bitfields, register_structs,
    registers::{ReadOnly, ReadWrite, WriteOnly},
};

register_bitfields![u32,
    UARTFR [
        BUSY OFFSET(3) NUMBITS(1) [],
        TXFF OFFSET(5) NUMBITS(1) []
    ],
    UARTIBRD [DIVISOR OFFSET(0) NUMBITS(16) []],
    UARTFBRD [DIVISOR OFFSET(0) NUMBITS(6) []],
    UARTLCR_H [
        FEN OFFSET(4) NUMBITS(1) [],
        WLEN OFFSET(5) NUMBITS(2) [EightBits = 3]
    ],
    UARTCR [
        UARTEN OFFSET(0) NUMBITS(1) [],
        TXE OFFSET(8) NUMBITS(1) []
    ],
    UARTInterrupts [ALL OFFSET(0) NUMBITS(11) []]
];

register_structs! {
    Pl011UartRegs {
        (0x00 => dr: ReadWrite<u32>),
        (0x04 => _reserved0),
        (0x18 => fr: ReadOnly<u32, UARTFR::Register>),
        (0x1c => _reserved1),
        (0x24 => ibrd: ReadWrite<u32, UARTIBRD::Register>),
        (0x28 => fbrd: ReadWrite<u32, UARTFBRD::Register>),
        (0x2c => lcr_h: ReadWrite<u32, UARTLCR_H::Register>),
        (0x30 => cr: ReadWrite<u32, UARTCR::Register>),
        (0x34 => ifls: ReadWrite<u32>),
        (0x38 => imsc: ReadWrite<u32, UARTInterrupts::Register>),
        (0x3c => ris: ReadOnly<u32>),
        (0x40 => mis: ReadOnly<u32>),
        (0x44 => icr: WriteOnly<u32, UARTInterrupts::Register>),
        (0x48 => @END),
    }
}

const POLL_LIMIT: usize = 100_000;
// QEMU virt: 24 MHz / (16 * 115200) = 13 + 1/64 (rounded).
const BAUD_INTEGER: u32 = 13;
const BAUD_FRACTION: u32 = 1;

pub(super) struct Pl011Uart {
    base: NonNull<Pl011UartRegs>,
}

impl Pl011Uart {
    /// # Safety
    /// `base` must be aligned, mapped Device memory for a PL011. The caller
    /// serializes access; this handle does not provide inter-thread locking.
    pub(super) unsafe fn new(base: *mut u8) -> Self {
        Self {
            base: NonNull::new(base).expect("null UART base").cast(),
        }
    }

    fn regs(&self) -> &Pl011UartRegs {
        // SAFETY: the constructor's mapping contract holds for this handle.
        unsafe { self.base.as_ref() }
    }

    pub(super) fn init(&mut self) -> bool {
        let regs = self.regs();
        // Let firmware TX finish before changing the line format.
        for _ in 0..POLL_LIMIT {
            if !regs.fr.is_set(UARTFR::BUSY) {
                regs.cr.set(0);
                regs.imsc.write(UARTInterrupts::ALL::CLEAR);
                regs.icr.write(UARTInterrupts::ALL::SET);
                regs.ibrd.write(UARTIBRD::DIVISOR.val(BAUD_INTEGER));
                regs.fbrd.write(UARTFBRD::DIVISOR.val(BAUD_FRACTION));
                regs.lcr_h
                    .write(UARTLCR_H::WLEN::EightBits + UARTLCR_H::FEN::SET);
                regs.cr.write(UARTCR::UARTEN::SET + UARTCR::TXE::SET);
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    pub(super) fn putchar(&mut self, byte: u8) -> Result<(), core::fmt::Error> {
        let regs = self.regs();
        for _ in 0..POLL_LIMIT {
            if !regs.fr.is_set(UARTFR::TXFF) {
                regs.dr.set(byte.into());
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(core::fmt::Error)
    }
}
