//! Checked newc record decoding; no boot file names or ordering policy.
use crate::archive::{ArchiveError, ArchiveErrorKind};
const HEADER_SIZE: usize = 110;
const ALIGNMENT: usize = 4;

/// Owns checked cursor movement; parsing never dereferences archive pointers.
pub struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Cursor<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }
    pub fn offset(&self) -> usize {
        self.offset
    }

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
    pub fn read_record(&mut self) -> Result<Record<'a>, ArchiveError> {
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
pub struct Entry<'a> {
    pub(super) name: &'a [u8],
    pub(super) data: &'a [u8],
}
pub enum Record<'a> {
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
mod tests {
    use super::*;

    #[test]
    fn cursor_alignment_and_overflow_are_checked_without_advancing() {
        let mut cursor = Cursor {
            bytes: &[0; 3],
            offset: 1,
        };
        assert_eq!(
            cursor.align().unwrap_err().kind,
            ArchiveErrorKind::Truncated
        );
        assert_eq!(cursor.offset, 1);
        assert_eq!(
            cursor.take(usize::MAX).unwrap_err().kind,
            ArchiveErrorKind::Overflow
        );
        assert_eq!(cursor.offset, 1);
        assert_eq!(hex(b"aBcD", 0).unwrap(), 0xabcd);
    }

    #[test]
    fn hex_requires_digits_and_reports_numeric_overflow() {
        for bytes in [
            &b"+0000001"[..],
            b"-0000001",
            b" 0000001",
            b"\xff0000001",
            b"",
        ] {
            assert_eq!(
                hex(bytes, 54).unwrap_err(),
                ArchiveError {
                    offset: 54,
                    kind: ArchiveErrorKind::InvalidHex,
                }
            );
        }
        assert_eq!(hex(b"00g0", 54).unwrap_err().offset, 56);
        let overflow = "f".repeat(core::mem::size_of::<usize>() * 2 + 1);
        assert_eq!(
            hex(overflow.as_bytes(), 54).unwrap_err(),
            ArchiveError {
                offset: 54,
                kind: ArchiveErrorKind::Overflow,
            }
        );
    }
}

#[test]
fn cursor_alignment_and_overflow_are_checked_without_advancing() {
    let mut cursor = Cursor {
        bytes: &[0; 3],
        offset: 1,
    };
    assert_eq!(
        cursor.align().unwrap_err().kind,
        ArchiveErrorKind::Truncated
    );
    assert_eq!(cursor.offset, 1);
    assert_eq!(
        cursor.take(usize::MAX).unwrap_err().kind,
        ArchiveErrorKind::Overflow
    );
    assert_eq!(cursor.offset, 1);
    assert_eq!(hex(b"aBcD", 0).unwrap(), 0xabcd);
}

#[test]
fn hex_requires_digits_and_reports_numeric_overflow() {
    for bytes in [
        &b"+0000001"[..],
        b"-0000001",
        b" 0000001",
        b"\xff0000001",
        b"",
    ] {
        assert_eq!(
            hex(bytes, 54).unwrap_err(),
            ArchiveError {
                offset: 54,
                kind: ArchiveErrorKind::InvalidHex,
            }
        );
    }
    assert_eq!(hex(b"00g0", 54).unwrap_err().offset, 56);
    let overflow = "f".repeat(core::mem::size_of::<usize>() * 2 + 1);
    assert_eq!(
        hex(overflow.as_bytes(), 54).unwrap_err(),
        ArchiveError {
            offset: 54,
            kind: ArchiveErrorKind::Overflow,
        }
    );
}
