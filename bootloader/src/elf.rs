//! Boot-specific image policy and physical loading. Parsing lives in rstiny-elf.
pub const PAGE: usize = rstiny_elf::PAGE_SIZE;

pub use rstiny_elf::page_up;

pub struct Elf<'a> {
    bytes: &'a [u8],
    image: rstiny_elf::Elf<'a>,
    pub headers: &'a [u8],
    pub count: usize,
    pub start: usize,
    pub end: usize,
    pub entry: usize,
}
impl<'a> Elf<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, rstiny_elf::Error> {
        let image = rstiny_elf::Elf::parse(bytes)?;
        if image
            .segments()
            .any(|s| !matches!(s.flags, 4..=6) || s.end - s.va > 32 * 1024 * 1024)
        {
            return Err(rstiny_elf::Error::InvalidSegment);
        }
        Ok(Self {
            bytes,
            headers: image.program_headers(),
            count: image.program_header_count(),
            start: image.start(),
            end: image.end(),
            entry: image.entry(),
            image,
        })
    }
    pub fn segments(&self) -> impl Iterator<Item = rstiny_elf::Segment> + '_ {
        self.image.segments()
    }
    /// # Safety
    /// The complete destination span must be exclusively owned writable RAM,
    /// disjoint from the loader/archive, with all arithmetic fitting that span.
    pub unsafe fn load(&self, destination: usize) {
        // SAFETY: The caller owns the destination span and excludes source
        // overlap. Parsing validated every source extent and segment offset.
        unsafe {
            core::ptr::write_bytes(destination as *mut u8, 0, self.end - self.start);
            for segment in self.segments() {
                core::ptr::copy_nonoverlapping(
                    self.bytes.as_ptr().add(segment.offset),
                    (destination + (segment.va - self.start)) as *mut u8,
                    segment.filesz,
                );
            }
        }
    }
}
