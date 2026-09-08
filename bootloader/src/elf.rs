//! ELF64 validation before any destination memory is modified.
pub const PAGE: usize = 4096;
pub const OFFSET: usize = 0xffff_0000_0000_0000;

#[derive(Clone, Copy, Default)]
pub struct Segment {
    pub flags: usize,
    pub offset: usize,
    pub va: usize,
    pub pa: usize,
    pub filesz: usize,
    pub memsz: usize,
    pub end: usize,
}
pub struct Elf<'a> {
    pub bytes: &'a [u8],
    pub headers: &'a [u8],
    pub count: usize,
    pub start: usize,
    pub end: usize,
    pub entry: usize,
    segments: [Segment; 32],
    loads: usize,
}
fn integer(data: &[u8], start: usize, size: usize) -> Result<usize, &'static str> {
    let bytes = data
        .get(start..start.checked_add(size).ok_or("ELF overflow")?)
        .ok_or("truncated ELF")?;
    Ok(bytes
        .iter()
        .enumerate()
        .fold(0, |value, (i, byte)| value | ((*byte as usize) << (8 * i))))
}
pub fn page_up(value: usize) -> Result<usize, &'static str> {
    Ok(value
        .checked_add(PAGE - 1)
        .ok_or("page rounding overflow")?
        & !(PAGE - 1))
}
impl<'a> Elf<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, &'static str> {
        if bytes.get(..7) != Some(b"\x7fELF\x02\x01\x01")
            || integer(bytes, 16, 2)? != 2
            || integer(bytes, 18, 2)? != 183
            || integer(bytes, 20, 4)? != 1
            || integer(bytes, 52, 2)? != 64
            || integer(bytes, 54, 2)? != 56
        {
            return Err("unsupported ELF64 header");
        }
        let count = integer(bytes, 56, 2)?;
        if count == 0 || count > 32 {
            return Err("invalid ELF header count");
        }
        let phoff = integer(bytes, 32, 8)?;
        let headers = bytes
            .get(phoff..phoff.checked_add(count * 56).ok_or("ELF header overflow")?)
            .ok_or("truncated ELF headers")?;
        let mut elf = Self {
            bytes,
            headers,
            count,
            start: usize::MAX,
            end: 0,
            entry: integer(bytes, 24, 8)?,
            segments: [Segment::default(); 32],
            loads: 0,
        };
        for header in headers.as_chunks::<56>().0 {
            let kind = integer(header, 0, 4)?;
            if matches!(kind, 2 | 3 | 7) {
                return Err("dynamic ELF/TLS unsupported");
            }
            let memsz = integer(header, 40, 8)?;
            if kind != 1 || memsz == 0 {
                continue;
            }
            let mut segment = Segment {
                flags: integer(header, 4, 4)?,
                offset: integer(header, 8, 8)?,
                va: integer(header, 16, 8)?,
                pa: integer(header, 24, 8)?,
                filesz: integer(header, 32, 8)?,
                memsz,
                end: 0,
            };
            let align = integer(header, 48, 8)?;
            segment.end = page_up(
                segment
                    .va
                    .checked_add(memsz)
                    .ok_or("ELF address overflow")?,
            )?;
            if !matches!(segment.flags, 4..=6)
                || segment.filesz > memsz
                || segment
                    .offset
                    .checked_add(segment.filesz)
                    .is_none_or(|end| end > bytes.len())
                || segment.pa.checked_add(memsz).is_none()
                || !segment.va.is_multiple_of(PAGE)
                || !segment.offset.is_multiple_of(PAGE)
                || align < PAGE
                || !align.is_power_of_two()
                || segment.va % align != segment.offset % align
                || segment.end - segment.va > 32 * 1024 * 1024
            {
                return Err("invalid ELF segment");
            }
            if elf
                .segments()
                .iter()
                .any(|other| segment.va < other.end && other.va < segment.end)
            {
                return Err("overlapping ELF segments");
            }
            elf.start = elf.start.min(segment.va);
            elf.end = elf.end.max(segment.end);
            elf.segments[elf.loads] = segment;
            elf.loads += 1;
        }
        if !elf.entry.is_multiple_of(4)
            || !elf
                .segments()
                .iter()
                .any(|s| s.flags == 5 && s.va <= elf.entry && elf.entry < s.va + s.filesz)
        {
            return Err("invalid ELF entry");
        }
        Ok(elf)
    }
    pub fn segments(&self) -> &[Segment] {
        &self.segments[..self.loads]
    }
    /// Caller validates the complete destination span is exclusively owned RAM,
    /// disjoint from the loader/archive, and all arithmetic fits that span.
    pub unsafe fn load(&self, destination: usize) {
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
