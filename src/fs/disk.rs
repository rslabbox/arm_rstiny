use core::cmp;
use log::error;
use fatfs::{IoBase, Read, Write, Seek, SeekFrom};
use crate::drivers::virtio::blk::BLOCK_DEVICE;

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
            buf[read_len..read_len + bytes_to_copy].copy_from_slice(&trash[offset..offset + bytes_to_copy]);
            
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
            // 1. Read existing sector
             if let Err(e) = blk.read_blocks(sector as usize, &mut trash) {
                 error!("virtio-blk read-for-write error: {:?}", e);
                 return Err(IoError::WriteError);
            }
            
            // 2. Modify buffer
            let bytes_to_copy = cmp::min(buf.len() - written_len, SECTOR_SIZE as usize - offset);
            trash[offset..offset + bytes_to_copy].copy_from_slice(&buf[written_len..written_len + bytes_to_copy]);
            
            // 3. Write back
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