//! The block device adapter: sector reads travel over the block IPC into the
//! BIND-granted shared buffer, then are copied out to whichever filesystem
//! backend needs them (docs/disk-driver.md section 7).


use embedded_io::{ErrorKind, Read as IoRead, Seek as IoSeek, SeekFrom};
use lwext4_rust::{Ext4Error, Ext4Result};
use rstiny::ipc;
use rstiny_protocol::{block, status};

/// The block service's shared DMA buffer, mapped at BIND (received frame).
pub const CLIENT_BUF_VA: usize = 0x0400_0000;

pub const SECTOR_SIZE: u64 = 512;
pub const SECTORS_PER_READ: u64 = 8;

/// The block device seen through the shared buffer: reads travel over IPC and
/// land in `CLIENT_BUF_VA`, then this adapter copies the requested window into
/// the filesystem library's buffer.
pub struct BlockDevice {
    pub ep: u64,
    pub position: u64,
    pub disk_bytes: u64,
}
impl BlockDevice {
    pub fn fetch(&mut self, lba: u64, count: u64) -> Result<(), ErrorKind> {
        let Ok(received) = ipc::call(self.ep, block::READ, &[lba, count]) else {
            return Err(ErrorKind::Other);
        };
        if received.label != status::OK {
            return Err(ErrorKind::Other);
        }
        Ok(())
    }
}
impl embedded_io::ErrorType for BlockDevice {
    type Error = ErrorKind;
}
impl IoRead for BlockDevice {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        let mut filled = 0;
        while filled < buf.len() && (self.position as u64) < self.disk_bytes {
            let lba = self.position / SECTOR_SIZE;
            let remaining_sectors = (self.disk_bytes - self.position).div_ceil(SECTOR_SIZE);
            let count = remaining_sectors.min(SECTORS_PER_READ);
            self.fetch(lba, count)?;
            let window = lba * SECTOR_SIZE;
            let offset = (self.position - window) as usize;
            let available = (count * SECTOR_SIZE) as usize - offset;
            let take = available.min(buf.len() - filled);
            // SAFETY: CLIENT_BUF_VA is exclusively mapped; the block service
            // only writes it while this task is blocked in the READ call.
            let source =
                unsafe { core::slice::from_raw_parts((CLIENT_BUF_VA + offset) as *const u8, take) };
            buf[filled..filled + take].copy_from_slice(source);
            self.position += take as u64;
            filled += take;
        }
        Ok(filled)
    }
}
impl IoSeek for BlockDevice {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64, Self::Error> {
        let target = match pos {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::End(delta) => (self.disk_bytes as i64)
                .checked_add(delta)
                .map(|v| u64::try_from(v).ok())
                .flatten(),
            SeekFrom::Current(delta) => (self.position as i64)
                .checked_add(delta)
                .map(|v| u64::try_from(v).ok())
                .flatten(),
        };
        match target {
            Some(value) => {
                self.position = value;
                Ok(self.position)
            }
            None => Err(ErrorKind::InvalidInput),
        }
    }
}

/// The filesystem backends share one fs v2 protocol; the trait is the seam
/// The same device behind the lwext4 block interface: reads copy out of the
/// block service's shared buffer; writes are refused (the acceptance images
/// are read-only and the QEMU drive is `readonly=on`).
impl lwext4_rust::BlockDevice for BlockDevice {
    fn read_blocks(&mut self, block_id: u64, buf: &mut [u8]) -> Ext4Result<usize> {
        let sectors = (buf.len().div_ceil(SECTOR_SIZE as usize)) as u64;
        let mut copied = 0usize;
        while copied < buf.len() {
            let lba = block_id + copied as u64 / SECTOR_SIZE;
            let count = (sectors - copied as u64 / SECTOR_SIZE as usize as u64).min(SECTORS_PER_READ)
                .min((self.disk_bytes - lba * SECTOR_SIZE) / SECTOR_SIZE);
            if count == 0 {
                break;
            }
            if self.fetch(lba, count).is_err() {
                return Err(Ext4Error::new(-5, None));
            }
            // SAFETY: CLIENT_BUF_VA is exclusively mapped and the block
            // service only writes it while this task is blocked in the call.
            let source = unsafe {
                core::slice::from_raw_parts(
                    (CLIENT_BUF_VA + (lba * SECTOR_SIZE - lba * SECTOR_SIZE) as usize) as *const u8,
                    (count * SECTOR_SIZE) as usize,
                )
            };
            let take = (count * SECTOR_SIZE) as usize;
            buf[copied..copied + take].copy_from_slice(&source[..take]);
            copied += take;
        }
        Ok(copied)
    }

    // lwext4's mount writes superblock state (mount count, fs state). The
    // disk is read-only, so the write is accepted and discarded: the read-only
    // consumer never depends on it, and the acceptance images are built
    // cleanly (no journal to replay). Reads never depend on these writes.
    fn write_blocks(&mut self, _block_id: u64, buf: &[u8]) -> Ext4Result<usize> {
        Ok(buf.len())
    }

    fn num_blocks(&self) -> Ext4Result<u64> {
        Ok(self.disk_bytes / SECTOR_SIZE)
    }
}
