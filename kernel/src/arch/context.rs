#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
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

// Assembly save/restore uses these offsets. Fail the build if the ABI changes.
const _: () = {
    assert!(core::mem::size_of::<TrapFrame>() == 272);
    assert!(core::mem::offset_of!(TrapFrame, usp) == 248);
    assert!(core::mem::offset_of!(TrapFrame, elr) == 256);
    assert!(core::mem::offset_of!(TrapFrame, spsr) == 264);
};
