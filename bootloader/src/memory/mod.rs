//! Checked physical ranges, virtual mappings and reserved-range allocation.
mod placement;
mod region;
pub(crate) use placement::allocate;
pub(crate) use region::{ImageMapping, Region};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Overflow,
    InvalidRange,
    NoMemory,
    KernelWindow,
    RootLayout,
    HeaderPage,
    LoadMinimum,
}
