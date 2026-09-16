//! Filesystem backends behind the fs v2 protocol: the `FileSystem` trait is
//! the seam between the wire protocol and the on-disk format. FAT32 rides
//! hadris-fat; ext4 is read-only through lwext4 (journal-less,
//! cleanly-unmounted images; lwext4's mount-time superblock write is
//! absorbed by the block device - the read-only consumer never needs it).

use alloc::vec::Vec;

use embedded_io::{ErrorKind, SeekFrom};
use hadris_fat::sync::{FatVolume, FatVolumeReadExt, FileEntry};
use lwext4_rust::{DummyHal, Ext4Filesystem, FileAttr, InodeType};

use crate::blockdev::BlockDevice;


/// The filesystem backends share one fs v2 protocol; the trait is the seam
/// between the wire protocol and the on-disk format. FAT32 rides hadris-fat;
/// ext4 is read-only through lwext4 (journal-less, cleanly-unmounted images).
pub enum OpenFile {
    Fat(FileEntry),
    Ext4 { ino: u32, size: u64 },
}
impl OpenFile {
    pub fn size(&self) -> u64 {
        match self {
            OpenFile::Fat(entry) => entry.len(),
            OpenFile::Ext4 { size, .. } => *size,
        }
    }
    pub fn is_dir(&self) -> bool {
        match self {
            OpenFile::Fat(entry) => entry.is_directory(),
            OpenFile::Ext4 { .. } => false,
        }
    }
}

pub trait FileSystem {
    /// Resolve a root-directory file name; directories are not openable.
    fn lookup(&mut self, name: &[u8]) -> Option<OpenFile>;
    /// Read up to `buf.len()` bytes at `offset`.
    fn read_at(
        &mut self,
        file: &mut OpenFile,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, ErrorKind>;
    /// Every root-directory entry as (name, size, is-dir); the caller applies
    /// the wire-format filters (name length, pagination).
    fn read_dir(&mut self) -> Vec<(alloc::string::String, u64, bool)>;
}

pub struct FatFs {
    pub volume: FatVolume<BlockDevice>,
}
impl FileSystem for FatFs {
    fn lookup(&mut self, name: &[u8]) -> Option<OpenFile> {
        let dir = self.volume.root_dir();
        for entry in dir.entries() {
            let Ok(entry) = entry else { continue };
            // FAT names are case-insensitive: `hello` matches a short name
            // stored as `HELLO` (and vice versa). `entry.name()` yields the
            // long name when the directory carries an LFN record (P2.1).
            if entry.name().as_bytes().eq_ignore_ascii_case(name)
                && entry.as_entry().is_some_and(|file| file.is_file())
            {
                return Some(OpenFile::Fat(entry.as_entry()?.clone()));
            }
        }
        None
    }
    fn read_at(
        &mut self,
        file: &mut OpenFile,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, ErrorKind> {
        let OpenFile::Fat(entry) = file else {
            return Err(ErrorKind::InvalidInput);
        };
        let mut reader = self.volume.read_file(entry).map_err(|_| ErrorKind::Other)?;
        reader.seek(SeekFrom::Start(offset)).map_err(|_| ErrorKind::Other)?;
        reader.read(buf).map_err(|_| ErrorKind::Other)
    }
    fn read_dir(&mut self) -> Vec<(alloc::string::String, u64, bool)> {
        let dir = self.volume.root_dir();
        let mut entries = Vec::new();
        for entry in dir.entries() {
            let Ok(entry) = entry else { continue };
            let Some(file) = entry.as_entry() else { continue };
            entries.push((
                alloc::string::String::from_utf8_lossy(entry.name().as_bytes()).into_owned(),
                file.len(),
                file.is_directory(),
            ));
        }
        entries
    }
}

/// ext4 through lwext4, read-only: the image must be journal-less (built with
/// `mke2fs -O ^has_journal`) and cleanly unmounted, so mounting and serving
/// never write. Root is inode 2; all block reads travel the same block IPC
/// the FAT backend uses.
pub struct Ext4Fs {
    pub fs: Ext4Filesystem<DummyHal, BlockDevice>,
}
impl FileSystem for Ext4Fs {
    fn lookup(&mut self, name: &[u8]) -> Option<OpenFile> {
        // ext4 names are case-sensitive UTF-8 (unlike the FAT backend's
        // case-insensitive match).
        let Ok(text) = core::str::from_utf8(name) else {
            return None;
        };
        let mut found = self.fs.lookup(2, text).ok()?;
        let ino = found.entry().ino();
        let mut attr = FileAttr::default();
        self.fs.get_attr(ino, &mut attr).ok()?;
        if attr.node_type == InodeType::Directory {
            return None; // fs v2 opens regular files only
        }
        Some(OpenFile::Ext4 {
            ino,
            size: attr.size,
        })
    }
    fn read_at(
        &mut self,
        file: &mut OpenFile,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, ErrorKind> {
        let OpenFile::Ext4 { ino, .. } = file else {
            return Err(ErrorKind::InvalidInput);
        };
        self.fs.read_at(*ino, buf, offset).map_err(|_| ErrorKind::Other)
    }
    fn read_dir(&mut self) -> Vec<(alloc::string::String, u64, bool)> {
        let mut entries = Vec::new();
        let Ok(mut reader) = self.fs.read_dir(2, 0) else {
            return entries;
        };
        while let Some(entry) = reader.current() {
            let name = alloc::string::String::from_utf8_lossy(entry.name()).into_owned();
            let ino = entry.ino();
            let is_dir = entry.inode_type() == InodeType::Directory;
            let mut attr = FileAttr::default();
            let _ = self.fs.get_attr(ino, &mut attr);
            entries.push((name, attr.size, is_dir));
            if reader.step().is_err() {
                break;
            }
        }
        entries
    }
}

