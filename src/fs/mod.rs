mod ops;

#[cfg(feature = "fat32")]
mod fat32;

#[cfg(feature = "fs9p")]
mod fs9p;

use crate::hal::Mutex;
use alloc::string::String;
use lazy_static::lazy_static;

#[allow(unused)]
pub use ops::{
    FileHandle, OpenOptions, change_dir, close, create_file, current_dir, dir_remove, file_remove,
    file_truncate, link, list_dir, make_dir, mkdir, mount, open, read_file, read_link, umount,
    unlink, write_file,
};

#[cfg(feature = "fat32")]
pub use fat32::DiskIo;

#[cfg(feature = "fat32")]
pub(crate) type FS = fatfs::FileSystem<DiskIo, fatfs::NullTimeProvider, fatfs::LossyOemCpConverter>;

lazy_static! {
    pub static ref CWD: Mutex<String> = Mutex::new(String::from("/"));
}

#[cfg(feature = "fat32")]
lazy_static! {
    pub static ref FILESYSTEM: Mutex<Option<FS>> = Mutex::new(None);
}

#[cfg(feature = "fs9p")]
lazy_static! {
    pub static ref P9_SESSION: Mutex<Option<::fs9p::Session>> = Mutex::new(None);
}

#[cfg(all(feature = "fat32", feature = "fs9p"))]
compile_error!("fat32 and fs9p features are mutually exclusive; enable only one.");

#[cfg(not(any(feature = "fat32", feature = "fs9p")))]
compile_error!("Either fat32 or fs9p feature must be enabled.");

pub fn init() {
    if let Err(err) = mount() {
        log::warn!("Filesystem mount failed: {}", err);
    }
}
