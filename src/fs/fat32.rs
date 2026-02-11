use alloc::{collections::BTreeMap, format};
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp;
use core::cmp::min;
use fatfs::{IoBase, Read, Seek, SeekFrom, Write};
use log::error;

use crate::drivers::virtio::blk::BLOCK_DEVICE;
use super::ops::{resolve_path, set_cwd, FileHandle, FsOps, OpenOptions};

/// A wrapper struct needed by fatfs to access the disk.
/// It maintains a current seek position/cursor.
pub struct DiskIo {
    pos: u64,
}

impl DiskIo {
    pub fn new() -> Self {
        Self { pos: 0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoError {
    ReadError,
    WriteError,
    SeekError,
    UnexpectedEof,
}

// Implement the strict error type required by fatfs
impl fatfs::IoError for IoError {
    fn is_interrupted(&self) -> bool {
        false
    }

    fn new_unexpected_eof_error() -> Self {
        Self::UnexpectedEof
    }

    fn new_write_zero_error() -> Self {
        Self::WriteError
    }
}

impl IoBase for DiskIo {
    type Error = IoError;
}

const SECTOR_SIZE: u64 = 512;

impl Read for DiskIo {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        let mut blk_guard = BLOCK_DEVICE.lock();
        let blk = blk_guard.as_mut().ok_or(IoError::ReadError)?;

        let mut read_len = 0;
        let mut trash = [0u8; SECTOR_SIZE as usize];

        while read_len < buf.len() {
            let sector = self.pos / SECTOR_SIZE;
            let offset = (self.pos % SECTOR_SIZE) as usize;

            // Read sector
            if let Err(e) = blk.read_blocks(sector as usize, &mut trash) {
                error!("virtio-blk read error: {:?}", e);
                return Err(IoError::ReadError);
            }

            let bytes_to_copy = cmp::min(buf.len() - read_len, SECTOR_SIZE as usize - offset);
            buf[read_len..read_len + bytes_to_copy]
                .copy_from_slice(&trash[offset..offset + bytes_to_copy]);

            self.pos += bytes_to_copy as u64;
            read_len += bytes_to_copy;
        }

        Ok(read_len)
    }
}

impl Write for DiskIo {
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        let mut blk_guard = BLOCK_DEVICE.lock();
        let blk = blk_guard.as_mut().ok_or(IoError::WriteError)?;

        let mut written_len = 0;
        let mut trash = [0u8; SECTOR_SIZE as usize];

        while written_len < buf.len() {
            let sector = self.pos / SECTOR_SIZE;
            let offset = (self.pos % SECTOR_SIZE) as usize;

            // Read-Modify-Write
            if let Err(e) = blk.read_blocks(sector as usize, &mut trash) {
                error!("virtio-blk read-for-write error: {:?}", e);
                return Err(IoError::WriteError);
            }

            let bytes_to_copy = cmp::min(buf.len() - written_len, SECTOR_SIZE as usize - offset);
            trash[offset..offset + bytes_to_copy]
                .copy_from_slice(&buf[written_len..written_len + bytes_to_copy]);

            if let Err(e) = blk.write_blocks(sector as usize, &trash) {
                error!("virtio-blk write error: {:?}", e);
                return Err(IoError::WriteError);
            }

            self.pos += bytes_to_copy as u64;
            written_len += bytes_to_copy;
        }

        Ok(written_len)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl Seek for DiskIo {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64, Self::Error> {
        match pos {
            SeekFrom::Start(off) => self.pos = off,
            SeekFrom::Current(off) => self.pos = (self.pos as i64 + off) as u64,
            SeekFrom::End(off) => {
                let mut blk_guard = BLOCK_DEVICE.lock();
                let blk = blk_guard.as_mut().ok_or(IoError::SeekError)?;
                let capacity = blk.capacity() * SECTOR_SIZE;
                self.pos = (capacity as i64 + off) as u64;
            }
        }
        Ok(self.pos)
    }
}

struct Fat32OpenEntry {
    path: String,
    options: OpenOptions,
}

pub struct Fat32Backend {
    open_files: BTreeMap<FileHandle, Fat32OpenEntry>,
    next_handle: FileHandle,
}

impl Fat32Backend {
    pub fn new() -> Self {
        Self {
            open_files: BTreeMap::new(),
            next_handle: 1,
        }
    }

    fn alloc_handle(&mut self, entry: Fat32OpenEntry) -> FileHandle {
        let handle = self.next_handle;
        self.next_handle = self.next_handle.wrapping_add(1).max(1);
        self.open_files.insert(handle, entry);
        handle
    }

    fn lookup(&self, handle: FileHandle) -> Result<&Fat32OpenEntry, String> {
        self.open_files
            .get(&handle)
            .ok_or_else(|| String::from("invalid file handle"))
    }
}

impl FsOps for Fat32Backend {
    fn mount(&mut self) -> Result<(), String> {
        let disk = DiskIo::new();
        let options = fatfs::FsOptions::new().update_accessed_date(false);
        let fs = fatfs::FileSystem::new(disk, options)
            .map_err(|e| format!("Failed to mount FAT32: {:?}", e))?;
        *crate::fs::FILESYSTEM.lock() = Some(fs);
        set_cwd("/");
        Ok(())
    }

    fn umount(&mut self) -> Result<(), String> {
        *crate::fs::FILESYSTEM.lock() = None;
        self.open_files.clear();
        set_cwd("/");
        Ok(())
    }

    fn open(&mut self, path: &str, options: OpenOptions) -> Result<FileHandle, String> {
        let target_path = resolve_path(path);
        let rel_path = target_path.strip_prefix('/').unwrap_or(&target_path);
        let fs_guard = crate::fs::FILESYSTEM.lock();
        let fs = fs_guard.as_ref().ok_or("Filesystem not initialized")?;
        let root = fs.root_dir();
        let mut file = if options.create {
            root.create_file(rel_path)
                .map_err(|e| format!("Failed to create file: {:?}", e))?
        } else {
            root.open_file(rel_path)
                .map_err(|e| format!("Failed to open file: {:?}", e))?
        };

        if options.truncate {
            use fatfs::Seek;
            use fatfs::SeekFrom;
            file.seek(SeekFrom::Start(0))
                .map_err(|e| format!("Failed to seek: {:?}", e))?;
            file.truncate()
                .map_err(|e| format!("Failed to truncate file: {:?}", e))?;
        }

        Ok(self.alloc_handle(Fat32OpenEntry {
            path: target_path,
            options,
        }))
    }

    fn close(&mut self, handle: FileHandle) -> Result<(), String> {
        self.open_files
            .remove(&handle)
            .map(|_| ())
            .ok_or_else(|| String::from("invalid file handle"))
    }

    fn lsdir(&mut self, path: &str) -> Result<Vec<String>, String> {
        let target_path = resolve_path(path);
        let fs_guard = crate::fs::FILESYSTEM.lock();
        let fs = fs_guard.as_ref().ok_or("Filesystem not initialized")?;
        let root = fs.root_dir();
        let dir = if target_path == "/" {
            root
        } else {
            let rel_path = target_path.strip_prefix('/').unwrap_or(&target_path);
            root.open_dir(rel_path)
                .map_err(|e| format!("Failed to open dir: {:?}", e))?
        };

        let mut entries = Vec::new();
        for entry in dir.iter() {
            let e = entry.map_err(|e| format!("Error reading entry: {:?}", e))?;
            entries.push(e.file_name());
        }
        Ok(entries)
    }

    fn mkdir(&mut self, path: &str) -> Result<(), String> {
        let target_path = resolve_path(path);
        if target_path == "/" {
            return Err(String::from("Cannot create root directory"));
        }
        let rel_path = target_path.strip_prefix('/').unwrap_or(&target_path);
        let fs_guard = crate::fs::FILESYSTEM.lock();
        let fs = fs_guard.as_ref().ok_or("Filesystem not initialized")?;
        fs.root_dir()
            .create_dir(rel_path)
            .map_err(|e| format!("Failed to create dir: {:?}", e))?;
        Ok(())
    }

    fn read_file(&mut self, handle: FileHandle, offset: u64, len: usize) -> Result<Vec<u8>, String> {
        use fatfs::Read;
        use fatfs::Seek;
        use fatfs::SeekFrom;

        let entry = self.lookup(handle)?;
        let rel_path = entry.path.strip_prefix('/').unwrap_or(&entry.path);
        let fs_guard = crate::fs::FILESYSTEM.lock();
        let fs = fs_guard.as_ref().ok_or("Filesystem not initialized")?;
        let root = fs.root_dir();
        let mut file = root
            .open_file(rel_path)
            .map_err(|e| format!("Failed to open file: {:?}", e))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| format!("Failed to seek: {:?}", e))?;

        let mut out = Vec::new();
        if len == 0 {
            let mut buf = [0u8; 1024];
            loop {
                let read = file
                    .read(&mut buf)
                    .map_err(|e| format!("Failed to read file: {:?}", e))?;
                if read == 0 {
                    break;
                }
                out.extend_from_slice(&buf[..read]);
            }
        } else {
            let mut remaining = len;
            let mut buf = [0u8; 1024];
            while remaining > 0 {
                let take = min(remaining, buf.len());
                let read = file
                    .read(&mut buf[..take])
                    .map_err(|e| format!("Failed to read file: {:?}", e))?;
                if read == 0 {
                    break;
                }
                out.extend_from_slice(&buf[..read]);
                remaining -= read;
            }
        }
        Ok(out)
    }

    fn read_link(&mut self, _path: &str) -> Result<String, String> {
        Err(String::from("read_link is not supported on FAT32"))
    }

    fn create_file(&mut self, path: &str) -> Result<FileHandle, String> {
        let options = OpenOptions {
            read: true,
            write: true,
            create: true,
            truncate: true,
            append: false,
        };
        self.open(path, options)
    }

    fn write_file(&mut self, handle: FileHandle, offset: u64, data: &[u8]) -> Result<usize, String> {
        use fatfs::Seek;
        use fatfs::SeekFrom;
        use fatfs::Write;

        let entry = self.lookup(handle)?;
        let rel_path = entry.path.strip_prefix('/').unwrap_or(&entry.path);
        let fs_guard = crate::fs::FILESYSTEM.lock();
        let fs = fs_guard.as_ref().ok_or("Filesystem not initialized")?;
        let root = fs.root_dir();
        let mut file = if entry.options.create {
            root.create_file(rel_path)
                .map_err(|e| format!("Failed to create file: {:?}", e))?
        } else {
            root.open_file(rel_path)
                .map_err(|e| format!("Failed to open file: {:?}", e))?
        };

        if entry.options.append {
            file.seek(SeekFrom::End(0))
                .map_err(|e| format!("Failed to seek: {:?}", e))?;
        } else {
            file.seek(SeekFrom::Start(offset))
                .map_err(|e| format!("Failed to seek: {:?}", e))?;
        }

        file.write(data)
            .map_err(|e| format!("Failed to write file: {:?}", e))
    }

    fn link(&mut self, _target: &str, _link_path: &str) -> Result<(), String> {
        Err(String::from("link is not supported on FAT32"))
    }

    fn unlink(&mut self, path: &str) -> Result<(), String> {
        self.file_remove(path)
    }

    fn file_truncate(&mut self, handle: FileHandle, size: u64) -> Result<(), String> {
        use fatfs::Seek;
        use fatfs::SeekFrom;

        let entry = self.lookup(handle)?;
        let rel_path = entry.path.strip_prefix('/').unwrap_or(&entry.path);
        let fs_guard = crate::fs::FILESYSTEM.lock();
        let fs = fs_guard.as_ref().ok_or("Filesystem not initialized")?;
        let root = fs.root_dir();
        let mut file = root
            .open_file(rel_path)
            .map_err(|e| format!("Failed to open file: {:?}", e))?;
        file.seek(SeekFrom::Start(size))
            .map_err(|e| format!("Failed to seek: {:?}", e))?;
        file.truncate()
            .map_err(|e| format!("Failed to truncate file: {:?}", e))?;
        Ok(())
    }

    fn file_remove(&mut self, path: &str) -> Result<(), String> {
        let target_path = resolve_path(path);
        if target_path == "/" {
            return Err(String::from("Cannot remove root directory"));
        }
        let rel_path = target_path.strip_prefix('/').unwrap_or(&target_path);
        let fs_guard = crate::fs::FILESYSTEM.lock();
        let fs = fs_guard.as_ref().ok_or("Filesystem not initialized")?;
        fs.root_dir()
            .remove(rel_path)
            .map_err(|e| format!("Failed to remove file: {:?}", e))?;
        Ok(())
    }

    fn dir_remove(&mut self, path: &str) -> Result<(), String> {
        self.file_remove(path)
    }

    fn stat(&mut self, path: &str) -> Result<super::ops::FileMetadata, String> {
        use super::ops::{FileMetadata, FileType};
        let target_path = resolve_path(path);
        if target_path == "/" {
            return Ok(FileMetadata {
                file_type: FileType::Directory,
                size: 0,
                mode: 0o755,
                nlink: 1,
                uid: 0,
                gid: 0,
                atime: 0,
                mtime: 0,
                ctime: 0,
            });
        }
        let rel_path = target_path.strip_prefix('/').unwrap_or(&target_path);
        let fs_guard = crate::fs::FILESYSTEM.lock();
        let fs = fs_guard.as_ref().ok_or("Filesystem not initialized")?;
        let root = fs.root_dir();
        // Try opening as directory first
        if root.open_dir(rel_path).is_ok() {
            return Ok(FileMetadata {
                file_type: FileType::Directory,
                size: 0,
                mode: 0o755,
                nlink: 1,
                uid: 0,
                gid: 0,
                atime: 0,
                mtime: 0,
                ctime: 0,
            });
        }
        // Try opening as file
        match root.open_file(rel_path) {
            Ok(_file) => Ok(FileMetadata {
                file_type: FileType::File,
                size: 0,
                mode: 0o644,
                nlink: 1,
                uid: 0,
                gid: 0,
                atime: 0,
                mtime: 0,
                ctime: 0,
            }),
            Err(e) => Err(format!("not found: {:?}", e)),
        }
    }

    fn rename(&mut self, _old_path: &str, _new_path: &str) -> Result<(), String> {
        Err(String::from("rename is not supported on FAT32"))
    }

    fn symlink(&mut self, _target: &str, _link_path: &str) -> Result<(), String> {
        Err(String::from("symlink is not supported on FAT32"))
    }

    fn chmod(&mut self, _path: &str, _mode: u32) -> Result<(), String> {
        Err(String::from("chmod is not supported on FAT32"))
    }

    fn readdir(&mut self, path: &str) -> Result<Vec<super::ops::DirEntry>, String> {
        use super::ops::{DirEntry, FileType};
        let target_path = resolve_path(path);
        let fs_guard = crate::fs::FILESYSTEM.lock();
        let fs = fs_guard.as_ref().ok_or("Filesystem not initialized")?;
        let root = fs.root_dir();
        let dir = if target_path == "/" {
            root
        } else {
            let rel_path = target_path.strip_prefix('/').unwrap_or(&target_path);
            root.open_dir(rel_path)
                .map_err(|e| format!("Failed to open dir: {:?}", e))?
        };
        let mut entries = Vec::new();
        for entry in dir.iter() {
            let e = entry.map_err(|e| format!("Error reading entry: {:?}", e))?;
            let file_type = if e.is_dir() {
                FileType::Directory
            } else {
                FileType::File
            };
            entries.push(DirEntry {
                name: e.file_name(),
                file_type,
            });
        }
        Ok(entries)
    }

    fn fsync(&mut self, _handle: FileHandle) -> Result<(), String> {
        // FAT32 via fatfs has no explicit fsync; writes go through immediately.
        Ok(())
    }
}
