use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use log::info;

use super::ops::{FileHandle, FsOps, OpenOptions, resolve_path, set_cwd};
use crate::drivers::virtio::p9::P9_DEVICE;

struct VirtioTransport;

impl fs9p::Transport for VirtioTransport {
    fn request(&self, req: &[u8], resp: &mut [u8]) -> Result<usize, String> {
        let mut guard = P9_DEVICE.lock();
        let dev = guard.as_mut().ok_or("virtio-9p not initialized")?;
        dev.request(req, resp)
            .map_err(|e| format!("virtio-9p error: {:?}", e))
    }
}

pub fn create_session() -> Result<fs9p::Session, String> {
    let mount_tag = p9_mount_tag().ok_or_else(|| String::from("virtio-9p device not available"))?;
    let mut session = fs9p::Session::new(Box::new(VirtioTransport), mount_tag.clone());
    session.negotiate()?;
    Ok(session)
}

fn p9_mount_tag() -> Option<String> {
    let guard = P9_DEVICE.lock();
    guard.as_ref().map(|dev| dev.mount_tag().to_string())
}

pub struct P9Backend;

impl P9Backend {
    fn map_open_options(options: OpenOptions) -> (u8, u32) {
        const OREAD: u8 = 0;
        const OWRITE: u8 = 1;
        const ORDWR: u8 = 2;
        const OTRUNC: u8 = 0x10;
        const OAPPEND: u8 = 0x80;

        const P9_DOTL_RDONLY: u32 = 0;
        const P9_DOTL_WRONLY: u32 = 1;
        const P9_DOTL_RDWR: u32 = 2;
        const P9_DOTL_CREATE: u32 = 0x100;
        const P9_DOTL_TRUNC: u32 = 0x1000;
        const P9_DOTL_APPEND: u32 = 0x2000;

        let (mut mode_9p, mut mode_dotl) = if options.read && options.write {
            (ORDWR, P9_DOTL_RDWR)
        } else if options.write {
            (OWRITE, P9_DOTL_WRONLY)
        } else {
            (OREAD, P9_DOTL_RDONLY)
        };

        if options.truncate {
            mode_9p |= OTRUNC;
            mode_dotl |= P9_DOTL_TRUNC;
        }
        if options.append {
            mode_9p |= OAPPEND;
            mode_dotl |= P9_DOTL_APPEND;
        }
        if options.create {
            mode_dotl |= P9_DOTL_CREATE;
        }

        (mode_9p, mode_dotl)
    }

    fn with_session<T>(
        f: impl FnOnce(&mut fs9p::Session) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut guard = crate::fs::P9_SESSION.lock();
        let session = guard.as_mut().ok_or("9p session not initialized")?;
        f(session)
    }
}

impl FsOps for P9Backend {
    fn mount(&mut self) -> Result<(), String> {
        let session = create_session()?;
        let tag = session.mount_tag().to_string();
        *crate::fs::P9_SESSION.lock() = Some(session);
        info!("virtio-9p mounted at /{tag}");
        set_cwd("/");
        Ok(())
    }

    fn umount(&mut self) -> Result<(), String> {
        *crate::fs::P9_SESSION.lock() = None;
        set_cwd("/");
        Ok(())
    }

    fn open(&mut self, path: &str, options: OpenOptions) -> Result<FileHandle, String> {
        let target_path = resolve_path(path);
        if options.create {
            let (mode_9p, mode_dotl) = Self::map_open_options(options);
            return Self::with_session(|session| {
                session.create_file_with_flags(&target_path, mode_9p, mode_dotl, 0o644)
            });
        }
        let (mode_9p, mode_dotl) = Self::map_open_options(options);
        Self::with_session(|session| session.open_path_with_flags(&target_path, mode_9p, mode_dotl))
    }

    fn close(&mut self, handle: FileHandle) -> Result<(), String> {
        Self::with_session(|session| session.close_fid(handle))
    }

    fn lsdir(&mut self, path: &str) -> Result<Vec<String>, String> {
        let target_path = resolve_path(path);
        Self::with_session(|session| session.list_dir(&target_path))
    }

    fn mkdir(&mut self, path: &str) -> Result<(), String> {
        let target_path = resolve_path(path);
        Self::with_session(|session| session.create_dir(&target_path))
    }

    fn read_file(
        &mut self,
        handle: FileHandle,
        offset: u64,
        len: usize,
    ) -> Result<Vec<u8>, String> {
        let count = if len == 0 { u32::MAX } else { len as u32 };
        Self::with_session(|session| session.read_fid(handle, offset, count))
    }

    fn read_link(&mut self, path: &str) -> Result<String, String> {
        let target_path = resolve_path(path);
        Self::with_session(|session| session.read_link(&target_path))
    }

    fn create_file(&mut self, path: &str) -> Result<FileHandle, String> {
        let target_path = resolve_path(path);
        Self::with_session(|session| session.create_file(&target_path))
    }

    fn write_file(
        &mut self,
        handle: FileHandle,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, String> {
        Self::with_session(|session| session.write_fid(handle, offset, data))
    }

    fn link(&mut self, target: &str, link_path: &str) -> Result<(), String> {
        let target_path = resolve_path(target);
        let link_path = resolve_path(link_path);
        Self::with_session(|session| {
            match session.link(&target_path, &link_path) {
                Ok(()) => Ok(()),
                Err(err) => match session.symlink(&target_path, &link_path) {
                    Ok(()) => Ok(()),
                    Err(_) => Err(err),
                },
            }
        })
    }

    fn unlink(&mut self, path: &str) -> Result<(), String> {
        let target_path = resolve_path(path);
        Self::with_session(|session| session.remove_path(&target_path))
    }

    fn file_truncate(&mut self, handle: FileHandle, size: u64) -> Result<(), String> {
        Self::with_session(|session| session.truncate_fid(handle, size))
    }

    fn file_remove(&mut self, path: &str) -> Result<(), String> {
        let target_path = resolve_path(path);
        Self::with_session(|session| session.remove_path(&target_path))
    }

    fn dir_remove(&mut self, path: &str) -> Result<(), String> {
        let target_path = resolve_path(path);
        Self::with_session(|session| session.remove_path(&target_path))
    }

    fn stat(&mut self, path: &str) -> Result<super::ops::FileMetadata, String> {
        use super::ops::{FileMetadata, FileType};
        let target_path = resolve_path(path);
        Self::with_session(|session| {
            let attr = session.getattr(&target_path)?;
            let file_type = if attr.qid_type & 0x80 != 0 {
                FileType::Directory
            } else if attr.qid_type & 0x02 != 0 {
                FileType::Symlink
            } else {
                FileType::File
            };
            Ok(FileMetadata {
                file_type,
                size: attr.size,
                mode: attr.mode,
                nlink: attr.nlink,
                uid: attr.uid,
                gid: attr.gid,
                atime: attr.atime_sec,
                mtime: attr.mtime_sec,
                ctime: attr.ctime_sec,
            })
        })
    }

    fn rename(&mut self, old_path: &str, new_path: &str) -> Result<(), String> {
        let old = resolve_path(old_path);
        let new = resolve_path(new_path);
        Self::with_session(|session| session.rename_path(&old, &new))
    }

    fn symlink(&mut self, target: &str, link_path: &str) -> Result<(), String> {
        let target_path = resolve_path(target);
        let link = resolve_path(link_path);
        Self::with_session(|session| session.symlink(&target_path, &link))
    }

    fn chmod(&mut self, path: &str, mode: u32) -> Result<(), String> {
        let target_path = resolve_path(path);
        Self::with_session(|session| session.setattr_mode(&target_path, mode))
    }

    fn readdir(&mut self, path: &str) -> Result<Vec<super::ops::DirEntry>, String> {
        use super::ops::{DirEntry, FileType};
        let target_path = resolve_path(path);
        Self::with_session(|session| {
            let entries = session.list_dir_entries(&target_path)?;
            Ok(entries
                .into_iter()
                .map(|e| {
                    let file_type = match e.entry_type {
                        4 => FileType::Directory,
                        8 => FileType::File,
                        10 => FileType::Symlink,
                        _ => FileType::Other,
                    };
                    DirEntry {
                        name: e.name,
                        file_type,
                    }
                })
                .collect())
        })
    }

    fn fsync(&mut self, _handle: FileHandle) -> Result<(), String> {
        // QEMU's 9P local backend does not implement TFSYNC and crashes
        // if it receives this message type. Data written via virtio-9p
        // is passed through to the host filesystem synchronously, so
        // an explicit fsync is not needed.
        Ok(())
    }
}
