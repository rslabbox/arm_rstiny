#![allow(unused)]

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::fs::CWD;
use crate::hal::Mutex;

pub type FileHandle = u32;

#[derive(Clone, Copy, Debug, Default)]
pub struct OpenOptions {
    pub read: bool,
    pub write: bool,
    pub create: bool,
    pub truncate: bool,
    pub append: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileType {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Clone, Debug)]
pub struct FileMetadata {
    pub file_type: FileType,
    pub size: u64,
    pub mode: u32,
    pub nlink: u64,
    pub uid: u32,
    pub gid: u32,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
}

#[derive(Clone, Debug)]
pub struct DirEntry {
    pub name: String,
    pub file_type: FileType,
}

pub trait FsOps: Send {
    fn mount(&mut self) -> Result<(), String>;
    fn umount(&mut self) -> Result<(), String>;

    fn open(&mut self, path: &str, options: OpenOptions) -> Result<FileHandle, String>;
    fn close(&mut self, handle: FileHandle) -> Result<(), String>;

    fn lsdir(&mut self, path: &str) -> Result<Vec<String>, String>;
    fn mkdir(&mut self, path: &str) -> Result<(), String>;

    fn read_file(&mut self, handle: FileHandle, offset: u64, len: usize)
    -> Result<Vec<u8>, String>;
    fn read_link(&mut self, path: &str) -> Result<String, String>;
    fn create_file(&mut self, path: &str) -> Result<FileHandle, String>;
    fn write_file(&mut self, handle: FileHandle, offset: u64, data: &[u8])
    -> Result<usize, String>;
    fn link(&mut self, target: &str, link_path: &str) -> Result<(), String>;
    fn unlink(&mut self, path: &str) -> Result<(), String>;
    fn file_truncate(&mut self, handle: FileHandle, size: u64) -> Result<(), String>;
    fn file_remove(&mut self, path: &str) -> Result<(), String>;
    fn dir_remove(&mut self, path: &str) -> Result<(), String>;

    fn stat(&mut self, path: &str) -> Result<FileMetadata, String>;
    fn rename(&mut self, old_path: &str, new_path: &str) -> Result<(), String>;
    fn symlink(&mut self, target: &str, link_path: &str) -> Result<(), String>;
    fn chmod(&mut self, path: &str, mode: u32) -> Result<(), String>;
    fn readdir(&mut self, path: &str) -> Result<Vec<DirEntry>, String>;
    fn fsync(&mut self, handle: FileHandle) -> Result<(), String>;
}

pub(crate) fn resolve_path(path: &str) -> String {
    let cwd = CWD.lock().clone();
    let abs_path = if path.starts_with('/') {
        String::from(path)
    } else if cwd.ends_with('/') {
        format!("{}{}", cwd, path)
    } else {
        format!("{}/{}", cwd, path)
    };

    let mut parts = Vec::new();
    for part in abs_path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            parts.pop();
        } else {
            parts.push(part);
        }
    }

    let mut res = String::from("/");
    res.push_str(&parts.join("/"));
    res
}

pub(crate) fn set_cwd(path: &str) {
    *CWD.lock() = path.to_string();
}

lazy_static::lazy_static! {
    static ref BACKEND: Mutex<Box<dyn FsOps>> = Mutex::new(select_backend());
}

fn select_backend() -> Box<dyn FsOps> {
    #[cfg(feature = "fat32")]
    {
        return Box::new(crate::fs::fat32::Fat32Backend::new());
    }
    #[cfg(feature = "fs9p")]
    {
        return Box::new(crate::fs::fs9p::P9Backend);
    }
    #[allow(unreachable_code)]
    Box::new(NullBackend)
}

struct NullBackend;

impl FsOps for NullBackend {
    fn mount(&mut self) -> Result<(), String> {
        Err(String::from("no filesystem backend selected"))
    }
    fn umount(&mut self) -> Result<(), String> {
        Err(String::from("no filesystem backend selected"))
    }
    fn open(&mut self, _path: &str, _options: OpenOptions) -> Result<FileHandle, String> {
        Err(String::from("no filesystem backend selected"))
    }
    fn close(&mut self, _handle: FileHandle) -> Result<(), String> {
        Err(String::from("no filesystem backend selected"))
    }
    fn lsdir(&mut self, _path: &str) -> Result<Vec<String>, String> {
        Err(String::from("no filesystem backend selected"))
    }
    fn mkdir(&mut self, _path: &str) -> Result<(), String> {
        Err(String::from("no filesystem backend selected"))
    }
    fn read_file(
        &mut self,
        _handle: FileHandle,
        _offset: u64,
        _len: usize,
    ) -> Result<Vec<u8>, String> {
        Err(String::from("no filesystem backend selected"))
    }
    fn read_link(&mut self, _path: &str) -> Result<String, String> {
        Err(String::from("no filesystem backend selected"))
    }
    fn create_file(&mut self, _path: &str) -> Result<FileHandle, String> {
        Err(String::from("no filesystem backend selected"))
    }
    fn write_file(
        &mut self,
        _handle: FileHandle,
        _offset: u64,
        _data: &[u8],
    ) -> Result<usize, String> {
        Err(String::from("no filesystem backend selected"))
    }
    fn link(&mut self, _target: &str, _link_path: &str) -> Result<(), String> {
        Err(String::from("no filesystem backend selected"))
    }
    fn unlink(&mut self, _path: &str) -> Result<(), String> {
        Err(String::from("no filesystem backend selected"))
    }
    fn file_truncate(&mut self, _handle: FileHandle, _size: u64) -> Result<(), String> {
        Err(String::from("no filesystem backend selected"))
    }
    fn file_remove(&mut self, _path: &str) -> Result<(), String> {
        Err(String::from("no filesystem backend selected"))
    }
    fn dir_remove(&mut self, _path: &str) -> Result<(), String> {
        Err(String::from("no filesystem backend selected"))
    }
    fn stat(&mut self, _path: &str) -> Result<FileMetadata, String> {
        Err(String::from("no filesystem backend selected"))
    }
    fn rename(&mut self, _old_path: &str, _new_path: &str) -> Result<(), String> {
        Err(String::from("no filesystem backend selected"))
    }
    fn symlink(&mut self, _target: &str, _link_path: &str) -> Result<(), String> {
        Err(String::from("no filesystem backend selected"))
    }
    fn chmod(&mut self, _path: &str, _mode: u32) -> Result<(), String> {
        Err(String::from("no filesystem backend selected"))
    }
    fn readdir(&mut self, _path: &str) -> Result<Vec<DirEntry>, String> {
        Err(String::from("no filesystem backend selected"))
    }
    fn fsync(&mut self, _handle: FileHandle) -> Result<(), String> {
        Err(String::from("no filesystem backend selected"))
    }
}

pub fn mount() -> Result<(), String> {
    BACKEND.lock().mount()
}

pub fn umount() -> Result<(), String> {
    BACKEND.lock().umount()
}

pub fn open(path: &str, options: OpenOptions) -> Result<FileHandle, String> {
    BACKEND.lock().open(path, options)
}

pub fn close(handle: FileHandle) -> Result<(), String> {
    BACKEND.lock().close(handle)
}

pub fn list_dir(path: Option<&str>) -> Result<(), String> {
    let target_path = resolve_path(path.unwrap_or(""));
    let entries = BACKEND.lock().lsdir(&target_path)?;
    for name in entries {
        println!("{}", name);
    }
    Ok(())
}

pub fn change_dir(path: &str) -> Result<(), String> {
    let target_path = resolve_path(path);
    BACKEND.lock().lsdir(&target_path)?;
    set_cwd(&target_path);
    Ok(())
}

pub fn mkdir(path: &str) -> Result<(), String> {
    let target_path = resolve_path(path);
    BACKEND.lock().mkdir(&target_path)
}

pub fn make_dir(path: &str) -> Result<(), String> {
    mkdir(path)
}

pub fn read_file(handle: FileHandle, offset: u64, len: usize) -> Result<Vec<u8>, String> {
    BACKEND.lock().read_file(handle, offset, len)
}

pub fn read_link(path: &str) -> Result<String, String> {
    let target_path = resolve_path(path);
    BACKEND.lock().read_link(&target_path)
}

pub fn create_file(path: &str) -> Result<FileHandle, String> {
    let target_path = resolve_path(path);
    BACKEND.lock().create_file(&target_path)
}

pub fn write_file(handle: FileHandle, offset: u64, data: &[u8]) -> Result<usize, String> {
    BACKEND.lock().write_file(handle, offset, data)
}

pub fn link(target: &str, link_path: &str) -> Result<(), String> {
    let target_path = resolve_path(target);
    let link_path = resolve_path(link_path);
    BACKEND.lock().link(&target_path, &link_path)
}

pub fn unlink(path: &str) -> Result<(), String> {
    let target_path = resolve_path(path);
    BACKEND.lock().unlink(&target_path)
}

pub fn file_truncate(handle: FileHandle, size: u64) -> Result<(), String> {
    BACKEND.lock().file_truncate(handle, size)
}

pub fn file_remove(path: &str) -> Result<(), String> {
    let target_path = resolve_path(path);
    BACKEND.lock().file_remove(&target_path)
}

pub fn dir_remove(path: &str) -> Result<(), String> {
    let target_path = resolve_path(path);
    BACKEND.lock().dir_remove(&target_path)
}

pub fn current_dir() -> String {
    CWD.lock().clone()
}

pub fn stat(path: &str) -> Result<FileMetadata, String> {
    let target_path = resolve_path(path);
    BACKEND.lock().stat(&target_path)
}

pub fn rename(old_path: &str, new_path: &str) -> Result<(), String> {
    let old = resolve_path(old_path);
    let new = resolve_path(new_path);
    BACKEND.lock().rename(&old, &new)
}

pub fn symlink(target: &str, link_path: &str) -> Result<(), String> {
    let target_path = resolve_path(target);
    let link = resolve_path(link_path);
    BACKEND.lock().symlink(&target_path, &link)
}

pub fn chmod(path: &str, mode: u32) -> Result<(), String> {
    let target_path = resolve_path(path);
    BACKEND.lock().chmod(&target_path, mode)
}

pub fn readdir(path: &str) -> Result<Vec<DirEntry>, String> {
    let target_path = resolve_path(path);
    BACKEND.lock().readdir(&target_path)
}

pub fn exists(path: &str) -> bool {
    stat(path).is_ok()
}

pub fn is_dir(path: &str) -> bool {
    stat(path)
        .map(|m| m.file_type == FileType::Directory)
        .unwrap_or(false)
}

pub fn is_file(path: &str) -> bool {
    stat(path)
        .map(|m| m.file_type == FileType::File)
        .unwrap_or(false)
}

pub fn file_size(path: &str) -> Result<u64, String> {
    stat(path).map(|m| m.size)
}

pub fn copy_file(src: &str, dst: &str) -> Result<u64, String> {
    let src_options = OpenOptions {
        read: true,
        ..Default::default()
    };
    let src_handle = open(src, src_options)?;
    let dst_handle = create_file(dst)?;
    let mut offset = 0u64;
    loop {
        let data = read_file(src_handle, offset, 4096)?;
        if data.is_empty() {
            break;
        }
        let written = write_file(dst_handle, offset, &data)?;
        offset += written as u64;
    }
    close(src_handle)?;
    close(dst_handle)?;
    Ok(offset)
}

/// Recursive mkdir — create all missing parent directories (like `mkdir -p`).
pub fn mkdir_all(path: &str) -> Result<(), String> {
    let resolved = resolve_path(path);
    if resolved == "/" {
        return Ok(());
    }
    // Collect each ancestor that needs creation.
    let parts: Vec<&str> = resolved
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let mut current = String::from("/");
    for part in parts {
        if current.ends_with('/') {
            current.push_str(part);
        } else {
            current.push('/');
            current.push_str(part);
        }
        if !exists(&current) {
            mkdir(&current)?;
        }
    }
    Ok(())
}

/// Recursively remove a directory and all its contents.
pub fn remove_all(path: &str) -> Result<(), String> {
    let resolved = resolve_path(path);
    if resolved == "/" {
        return Err(String::from("cannot remove root directory"));
    }
    let meta = stat(&resolved)?;
    if meta.file_type != FileType::Directory {
        return file_remove(&resolved);
    }
    // Remove children first
    let entries = readdir(&resolved)?;
    for entry in entries {
        let child = if resolved == "/" {
            format!("/{}", entry.name)
        } else {
            format!("{}/{}", resolved, entry.name)
        };
        if entry.file_type == FileType::Directory {
            remove_all(&child)?;
        } else {
            file_remove(&child)?;
        }
    }
    dir_remove(&resolved)
}

/// Walk a directory tree recursively, calling `visitor` for each entry.
/// The visitor receives `(full_path, &DirEntry)` and returns `true` to continue, `false` to stop.
pub fn walk<F>(path: &str, visitor: &mut F) -> Result<(), String>
where
    F: FnMut(&str, &DirEntry) -> bool,
{
    walk_inner(path, visitor)?;
    Ok(())
}

/// Returns Ok(true) to continue walking, Ok(false) if visitor requested early stop.
fn walk_inner<F>(path: &str, visitor: &mut F) -> Result<bool, String>
where
    F: FnMut(&str, &DirEntry) -> bool,
{
    let resolved = resolve_path(path);
    let entries = readdir(&resolved)?;
    for entry in entries {
        let child = if resolved == "/" {
            format!("/{}", entry.name)
        } else {
            format!("{}/{}", resolved, entry.name)
        };
        if !visitor(&child, &entry) {
            return Ok(false);
        }
        if entry.file_type == FileType::Directory {
            if !walk_inner(&child, visitor)? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Flush file data to persistent storage.
pub fn fsync(handle: FileHandle) -> Result<(), String> {
    BACKEND.lock().fsync(handle)
}
