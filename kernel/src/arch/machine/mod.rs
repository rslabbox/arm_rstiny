//! CPU and hardware operations; scheduling policy lives outside this module.
pub(crate) mod gic;
pub(crate) mod instructions;
pub(crate) mod mmu;
pub(crate) mod time;
pub(crate) mod timer;
