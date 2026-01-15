mod disk;
mod ops;

use alloc::string::String;
use lazy_static::lazy_static;
use crate::hal::Mutex;

pub use disk::DiskIo;
pub use ops::{list_dir, change_dir, make_dir, current_dir};

type FS = fatfs::FileSystem<DiskIo, fatfs::NullTimeProvider, fatfs::LossyOemCpConverter>;

lazy_static! {
    pub static ref FILESYSTEM: Mutex<Option<FS>> = Mutex::new(None);
    pub static ref CWD: Mutex<String> = Mutex::new(String::from("/"));
}

pub fn init() {
    let disk = DiskIo::new();
    let options = fatfs::FsOptions::new().update_accessed_date(false);
    match fatfs::FileSystem::new(disk, options) {
        Ok(fs) => {
            log::info!("FAT32 Filesystem mounted!");
            *FILESYSTEM.lock() = Some(fs);
        },
        Err(e) => {
            log::error!("Failed to mount FAT32: {:?}", e);
            log::warn!("Make sure disk.img is formatted with FAT32");
        }
    }
}

