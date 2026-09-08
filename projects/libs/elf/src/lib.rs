#![no_std]
//! Allocation-free parsing of static, little-endian AArch64 ELF64 images.
//!
//! Supports at most 32 program headers and page-aligned PT_LOAD segments using
//! 4 KiB pages. Dynamic linking and TLS are unsupported. Parsing validates file
//! extents, address arithmetic, page overlap and the executable entry point.
//! Callers own address placement, W^X policy and every memory write.
use core::ops::Range;

pub const PAGE_SIZE: usize = 4096;
const HEADER_SIZE: usize = 64;
/// Size in bytes of one ELF64 program header.
pub const PROGRAM_HEADER_SIZE: usize = 56;
const MAX_HEADERS: usize = 32;

/// Malformed input and unsupported image features never require a panic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Truncated,
    UnsupportedHeader,
    HeaderCount,
    HeaderTableSize,
    Overflow,
    UnsupportedFeature,
    InvalidSegment,
    OverlappingSegments,
    InvalidEntry,
}
impl Error {
    pub const fn message(self) -> &'static str {
        match self {
            Self::Truncated => "truncated ELF",
            Self::UnsupportedHeader => "unsupported ELF64 header",
            Self::HeaderCount => "invalid ELF header count",
            Self::HeaderTableSize => "invalid ELF program header table size",
            Self::Overflow => "ELF address overflow",
            Self::UnsupportedFeature => "dynamic ELF/TLS unsupported",
            Self::InvalidSegment => "invalid ELF segment",
            Self::OverlappingSegments => "overlapping ELF segments",
            Self::InvalidEntry => "invalid ELF entry",
        }
    }
}
impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.message())
    }
}
impl core::error::Error for Error {}

fn integer(data: &[u8], offset: usize, size: usize) -> Result<usize, Error> {
    let bytes = data
        .get(offset..offset.checked_add(size).ok_or(Error::Overflow)?)
        .ok_or(Error::Truncated)?;
    let mut value = 0u64;
    for (i, byte) in bytes.iter().enumerate() {
        value |= (*byte as u64) << (8 * i);
    }
    usize::try_from(value).map_err(|_| Error::Overflow)
}

pub fn page_up(value: usize) -> Result<usize, Error> {
    Ok(value.checked_add(PAGE_SIZE - 1).ok_or(Error::Overflow)? & !(PAGE_SIZE - 1))
}

/// Parsed file header. Read `program_headers_range()` next when using file I/O.
#[derive(Clone, Copy, Debug)]
pub struct Header {
    offset: usize,
    count: usize,
    entry: usize,
}
impl Header {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < HEADER_SIZE {
            return Err(Error::Truncated);
        }
        if bytes.get(..7) != Some(b"\x7fELF\x02\x01\x01")
            || integer(bytes, 16, 2)? != 2
            || integer(bytes, 18, 2)? != 183
            || integer(bytes, 20, 4)? != 1
            || integer(bytes, 52, 2)? != HEADER_SIZE
            || integer(bytes, 54, 2)? != PROGRAM_HEADER_SIZE
        {
            return Err(Error::UnsupportedHeader);
        }
        let count = integer(bytes, 56, 2)?;
        if !(1..=MAX_HEADERS).contains(&count) {
            return Err(Error::HeaderCount);
        }
        let offset = integer(bytes, 32, 8)?;
        offset
            .checked_add(count * PROGRAM_HEADER_SIZE)
            .ok_or(Error::Overflow)?;
        Ok(Self {
            offset,
            count,
            entry: integer(bytes, 24, 8)?,
        })
    }

    pub fn program_headers_range(&self) -> Range<usize> {
        self.offset..self.offset + self.count * PROGRAM_HEADER_SIZE
    }
}

/// A decoded PT_LOAD record. `end` is the page-rounded virtual memory end.
#[derive(Clone, Copy, Debug)]
pub struct Segment {
    pub flags: usize,
    pub offset: usize,
    pub va: usize,
    pub pa: usize,
    pub filesz: usize,
    pub memsz: usize,
    pub end: usize,
}
fn segment(header: &[u8]) -> Result<Segment, Error> {
    let va = integer(header, 16, 8)?;
    let memsz = integer(header, 40, 8)?;
    Ok(Segment {
        flags: integer(header, 4, 4)?,
        offset: integer(header, 8, 8)?,
        va,
        pa: integer(header, 24, 8)?,
        filesz: integer(header, 32, 8)?,
        memsz,
        end: page_up(va.checked_add(memsz).ok_or(Error::Overflow)?)?,
    })
}

/// Validated metadata borrowing the program header bytes; no heap or segment array.
#[derive(Debug)]
pub struct Elf<'a> {
    headers: &'a [u8],
    start: usize,
    end: usize,
    entry: usize,
}
impl<'a> Elf<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        let header = Header::parse(bytes)?;
        let table = bytes
            .get(header.program_headers_range())
            .ok_or(Error::Truncated)?;
        Self::from_headers(header, table, bytes.len())
    }

    /// Validate a separately read program header table against the file size.
    /// The caller must read this table from `header.program_headers_range()` in
    /// the same immutable file whose header was parsed, and later copy segment
    /// bytes from that file. No segment payload is needed during validation.
    pub fn from_headers(header: Header, table: &'a [u8], file_size: usize) -> Result<Self, Error> {
        if table.len() != header.count * PROGRAM_HEADER_SIZE {
            return Err(Error::HeaderTableSize);
        }
        if header.program_headers_range().end > file_size || file_size < HEADER_SIZE {
            return Err(Error::Truncated);
        }
        let mut elf = Self {
            headers: table,
            start: usize::MAX,
            end: 0,
            entry: header.entry,
        };
        let mut entry_valid = false;
        for (index, raw) in table
            .as_chunks::<PROGRAM_HEADER_SIZE>()
            .0
            .iter()
            .enumerate()
        {
            let kind = integer(raw, 0, 4)?;
            if matches!(kind, 2 | 3 | 7) {
                return Err(Error::UnsupportedFeature);
            }
            if kind != 1 {
                continue;
            }
            let s = segment(raw)?;
            let align = integer(raw, 48, 8)?;
            if s.filesz > s.memsz
                || s.offset
                    .checked_add(s.filesz)
                    .is_none_or(|end| end > file_size)
                || s.pa.checked_add(s.memsz).is_none()
            {
                return Err(Error::InvalidSegment);
            }
            if s.memsz == 0 {
                continue;
            }
            if s.flags & !7 != 0
                || !s.va.is_multiple_of(PAGE_SIZE)
                || !s.offset.is_multiple_of(PAGE_SIZE)
                || align < PAGE_SIZE
                || !align.is_power_of_two()
                || s.va % align != s.offset % align
            {
                return Err(Error::InvalidSegment);
            }
            for previous in table[..index * PROGRAM_HEADER_SIZE]
                .as_chunks::<PROGRAM_HEADER_SIZE>()
                .0
            {
                if integer(previous, 0, 4)? == 1 && integer(previous, 40, 8)? != 0 {
                    let other = segment(previous)?;
                    if s.va < other.end && other.va < s.end {
                        return Err(Error::OverlappingSegments);
                    }
                }
            }
            elf.start = elf.start.min(s.va);
            elf.end = elf.end.max(s.end);
            entry_valid |= s.flags & 1 != 0 && s.va <= elf.entry && elf.entry < s.va + s.filesz;
        }
        if !entry_valid || !elf.entry.is_multiple_of(4) {
            return Err(Error::InvalidEntry);
        }
        Ok(elf)
    }

    pub fn start(&self) -> usize {
        self.start
    }
    pub fn end(&self) -> usize {
        self.end
    }
    pub fn entry(&self) -> usize {
        self.entry
    }
    /// Original table, including non-load headers, for the seL4 boot handoff.
    pub fn program_headers(&self) -> &'a [u8] {
        self.headers
    }
    pub fn program_header_count(&self) -> usize {
        self.headers.len() / PROGRAM_HEADER_SIZE
    }
    pub fn segments(&self) -> impl Iterator<Item = Segment> + '_ {
        self.headers
            .as_chunks::<PROGRAM_HEADER_SIZE>()
            .0
            .iter()
            .filter(|h| integer(*h, 0, 4) == Ok(1) && integer(*h, 40, 8) != Ok(0))
            // Immutable headers were fully validated by the constructors.
            .map(|h| segment(h).expect("validated ELF segment"))
    }
}
