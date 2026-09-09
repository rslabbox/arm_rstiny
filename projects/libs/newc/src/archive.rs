//! Ordered boot archive contract, built on checked newc decoding.
use crate::newc::{Cursor, Record};
use core::fmt;

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
    DuplicateModule,
    TooManyModules,
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

/// Maximum number of boot modules carried after the three boot images.
pub const MAX_MODULES: usize = 16;

/// Validated archive structure and file order, borrowing the original payloads.
/// ELF and DTB contents must still be validated by their respective parsers.
/// Files after `rootserver` are boot modules passed through to the root task.
#[derive(Debug)]
pub struct BootArchive<'a> {
    kernel: &'a [u8],
    device_tree: &'a [u8],
    rootserver: &'a [u8],
    modules: [(&'a [u8], &'a [u8]); MAX_MODULES],
    module_count: usize,
}
impl<'a> BootArchive<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ArchiveError> {
        let mut cursor = Cursor::new(bytes);
        let kernel = expect_file(&mut cursor, b"kernel.elf")?;
        let device_tree = expect_file(&mut cursor, b"kernel.dtb")?;
        let rootserver = expect_file(&mut cursor, b"userboot")?;
        let mut modules = [(&[] as &[u8], &[] as &[u8]); MAX_MODULES];
        let mut module_count = 0;
        loop {
            if cursor.offset() == bytes.len() {
                return Err(ArchiveError {
                    offset: cursor.offset(),
                    kind: ArchiveErrorKind::MissingTrailer,
                });
            }
            let offset = cursor.offset();
            match cursor.read_record()? {
                Record::File(entry) => {
                    if module_count >= MAX_MODULES {
                        return Err(ArchiveError {
                            offset,
                            kind: ArchiveErrorKind::TooManyModules,
                        });
                    }
                    if entry.name.is_empty()
                        || modules[..module_count]
                            .iter()
                            .any(|(name, _)| *name == entry.name)
                        || [b"kernel.elf".as_slice(), b"kernel.dtb", b"userboot"]
                            .contains(&entry.name)
                    {
                        return Err(ArchiveError {
                            offset,
                            kind: ArchiveErrorKind::DuplicateModule,
                        });
                    }
                    modules[module_count] = (entry.name, entry.data);
                    module_count += 1;
                }
                Record::Trailer => break,
            }
        }
        // GNU cpio pads the archive to a block boundary. Only zero fill may
        // follow the terminal record; concatenated archives are not boot images.
        if let Some(index) = bytes[cursor.offset()..].iter().position(|byte| *byte != 0) {
            return Err(ArchiveError {
                offset: cursor.offset() + index,
                kind: ArchiveErrorKind::TrailingData,
            });
        }
        Ok(Self {
            kernel,
            device_tree,
            rootserver,
            modules,
            module_count,
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
    /// The boot modules after the three boot images, in archive order.
    pub fn modules(&self) -> &[(&'a [u8], &'a [u8])] {
        &self.modules[..self.module_count]
    }
}

fn expect_file<'a>(cursor: &mut Cursor<'a>, expected: &[u8]) -> Result<&'a [u8], ArchiveError> {
    let offset = cursor.offset();
    match cursor.read_record()? {
        Record::File(entry) if entry.name == expected => Ok(entry.data),
        _ => Err(ArchiveError {
            offset,
            kind: ArchiveErrorKind::UnexpectedFile,
        }),
    }
}
