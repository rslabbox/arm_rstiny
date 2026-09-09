//! Parse, plan, then commit the boot images; architecture code consumes Handoff.
mod commit;
mod error;
mod images;
#[cfg(not(test))]
mod linked;
mod plan;
pub(crate) use error::BootError;
#[cfg(not(test))]
pub(crate) use linked::plan;
pub(crate) use plan::LoadPlan;

/// Loaded image metadata; only the root/DTB fields cross the six-register ABI.
pub(crate) struct Handoff {
    pub(crate) image_start: usize,
    pub(crate) image_end: usize,
    pub(crate) offset: usize,
    pub(crate) root_entry: usize,
    pub(crate) dtb: usize,
    pub(crate) dtb_size: usize,
    pub(crate) kernel_entry: usize,
    pub(crate) kernel_mapping: crate::memory::ImageMapping,
    pub(crate) modules: usize,
    pub(crate) modules_size: usize,
}
