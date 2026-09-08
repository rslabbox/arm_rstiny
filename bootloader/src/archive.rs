//! Borrowed newc records and the boot image's ordered three-file contract.
use core::fmt;

const HEADER_SIZE: usize = 110;
const ALIGNMENT: usize = 4;

/// A malformed archive location, measured in bytes from its beginning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArchiveError {
    pub offset: usize,
    pub kind: ArchiveErrorKind,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveErrorKind {
    Truncated,
    Overflow,
    InvalidMagic,
    InvalidHex,
    InvalidName,
    UnexpectedFile,
    MissingTrailer,
    InvalidTrailer,
    TrailingData,
}
impl fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CPIO {:?} at offset {:#x}", self.kind, self.offset)
    }
}
impl core::error::Error for ArchiveError {}

/// Validated archive structure and file order, borrowing the original payloads.
/// ELF and DTB contents must still be validated by their respective parsers.
#[derive(Debug)]
pub struct BootArchive<'a> {
    kernel: &'a [u8],
    device_tree: &'a [u8],
    rootserver: &'a [u8],
}
impl<'a> BootArchive<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ArchiveError> {
        let mut cursor = Cursor { bytes, offset: 0 };
        let kernel = cursor.expect_file(b"kernel.elf")?;
        let device_tree = cursor.expect_file(b"kernel.dtb")?;
        let rootserver = cursor.expect_file(b"rootserver")?;
        if cursor.offset == bytes.len() {
            return Err(cursor.error(ArchiveErrorKind::MissingTrailer));
        }
        let trailer_offset = cursor.offset;
        if !matches!(cursor.read_record()?, Record::Trailer) {
            return Err(ArchiveError {
                offset: trailer_offset,
                kind: ArchiveErrorKind::UnexpectedFile,
            });
        }
        // GNU cpio pads the archive to a block boundary. Only zero fill may
        // follow the terminal record; concatenated archives are not boot images.
        if let Some(index) = bytes[cursor.offset..].iter().position(|byte| *byte != 0) {
            return Err(ArchiveError {
                offset: cursor.offset + index,
                kind: ArchiveErrorKind::TrailingData,
            });
        }
        Ok(Self {
            kernel,
            device_tree,
            rootserver,
        })
    }
    pub fn kernel(&self) -> &'a [u8] {
        self.kernel
    }
    pub fn device_tree(&self) -> &'a [u8] {
        self.device_tree
    }
    pub fn rootserver(&self) -> &'a [u8] {
        self.rootserver
    }
}

/// Owns checked cursor movement; parsing never dereferences archive pointers.
struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Cursor<'a> {
    fn error(&self, kind: ArchiveErrorKind) -> ArchiveError {
        ArchiveError {
            offset: self.offset,
            kind,
        }
    }
    fn take(&mut self, len: usize) -> Result<&'a [u8], ArchiveError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| self.error(ArchiveErrorKind::Overflow))?;
        let data = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| self.error(ArchiveErrorKind::Truncated))?;
        self.offset = end;
        Ok(data)
    }
    fn align(&mut self) -> Result<(), ArchiveError> {
        let padding = (ALIGNMENT - self.offset % ALIGNMENT) % ALIGNMENT;
        self.take(padding)?;
        Ok(())
    }
    fn read_record(&mut self) -> Result<Record<'a>, ArchiveError> {
        let header_offset = self.offset;
        let header = NewcHeader::parse(self.take(HEADER_SIZE)?, header_offset)?;
        let name_offset = self.offset;
        let raw_name = self.take(header.name_size)?;
        let name = raw_name
            .strip_suffix(&[0])
            .filter(|name| !name.is_empty() && !name.contains(&0))
            .ok_or(ArchiveError {
                offset: name_offset,
                kind: ArchiveErrorKind::InvalidName,
            })?;
        self.align()?;
        if name == b"TRAILER!!!" && header.file_size != 0 {
            return Err(ArchiveError {
                offset: header_offset,
                kind: ArchiveErrorKind::InvalidTrailer,
            });
        }
        let data = self.take(header.file_size)?;
        self.align()?;
        Ok(if name == b"TRAILER!!!" {
            Record::Trailer
        } else {
            Record::File(Entry { name, data })
        })
    }
    fn expect_file(&mut self, expected: &[u8]) -> Result<&'a [u8], ArchiveError> {
        let offset = self.offset;
        match self.read_record()? {
            Record::File(entry) if entry.name == expected => Ok(entry.data),
            _ => Err(ArchiveError {
                offset,
                kind: ArchiveErrorKind::UnexpectedFile,
            }),
        }
    }
}

/// newc fields are ASCII hexadecimal, not a native binary structure.
struct NewcHeader {
    name_size: usize,
    file_size: usize,
}
impl NewcHeader {
    fn parse(bytes: &[u8], offset: usize) -> Result<Self, ArchiveError> {
        if bytes.get(..6) != Some(b"070701") {
            return Err(ArchiveError {
                offset,
                kind: ArchiveErrorKind::InvalidMagic,
            });
        }
        // These fields describe archive metadata that the bootloader does not
        // interpret, but malformed hexadecimal still makes the header invalid.
        let mut name_size = 0;
        let mut file_size = 0;
        for (index, field) in bytes[6..].as_chunks::<8>().0.iter().enumerate() {
            let value = hex(field, offset + 6 + index * 8)?;
            match index {
                6 => file_size = value,
                11 => name_size = value,
                _ => (),
            }
        }
        Ok(Self {
            name_size,
            file_size,
        })
    }
}
struct Entry<'a> {
    name: &'a [u8],
    data: &'a [u8],
}
enum Record<'a> {
    File(Entry<'a>),
    Trailer,
}
fn hex(bytes: &[u8], offset: usize) -> Result<usize, ArchiveError> {
    // CPIO permits only hexadecimal digits; from_str_radix also accepts '+'.
    if let Some(index) = bytes.iter().position(|byte| !byte.is_ascii_hexdigit()) {
        return Err(ArchiveError {
            offset: offset + index,
            kind: ArchiveErrorKind::InvalidHex,
        });
    }
    let text = core::str::from_utf8(bytes).map_err(|_| ArchiveError {
        offset,
        kind: ArchiveErrorKind::InvalidHex,
    })?;
    usize::from_str_radix(text, 16).map_err(|error| ArchiveError {
        offset,
        kind: match error.kind() {
            core::num::IntErrorKind::PosOverflow => ArchiveErrorKind::Overflow,
            _ => ArchiveErrorKind::InvalidHex,
        },
    })
}

#[cfg(test)]
#[path = "../tests/archive.rs"]
mod tests;
