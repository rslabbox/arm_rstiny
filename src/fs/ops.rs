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
