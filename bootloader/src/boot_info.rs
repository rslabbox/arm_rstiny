//! Loaded image metadata; only the root/DTB fields cross the six-register ABI.
pub(crate) struct Handoff {
    pub(crate) image_start: usize,
    pub(crate) image_end: usize,
    pub(crate) offset: usize,
    pub(crate) root_entry: usize,
    pub(crate) dtb: usize,
    pub(crate) dtb_size: usize,
    pub(crate) kernel_entry: usize,
    pub(crate) kernel_mapping: crate::layout::ImageMapping,
}
