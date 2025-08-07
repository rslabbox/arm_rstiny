use alloc::vec;
use alloc::vec::Vec;

use super::virtio::DefaultVirtioAlloc;
use crate::virtio::block::VirtioBlkDevice;
use fatfs::{Read, Seek, SeekFrom, Write};

pub struct MyFileSystem {
    blk_dev: VirtioBlkDevice<DefaultVirtioAlloc>,
    position: u64,
    sector_size: u64,
    sector_buffer: Vec<u8>,
    buffer_sector: Option<u64>,
    buffer_dirty: bool,
    immediate_write: bool, // 新增：控制是否立即写入
}

impl MyFileSystem {
    pub fn new(blk_dev: VirtioBlkDevice<DefaultVirtioAlloc>) -> Self {
        let sector_size = 512;
        MyFileSystem {
            blk_dev,
            position: 0,
            sector_size,
            sector_buffer: vec![0u8; sector_size as usize],
            buffer_sector: None,
            buffer_dirty: false,
            immediate_write: false, // 默认使用缓冲
        }
    }

    fn flush_buffer(&mut self) -> Result<(), ()> {
        if self.buffer_dirty {
            if let Some(sector) = self.buffer_sector {
                self.blk_dev
                    .write_sectors(sector, &self.sector_buffer)
                    .map_err(|_| ())?;
                self.blk_dev.flush().map_err(|_| ())?;
                self.buffer_dirty = false;
            }
        }
        Ok(())
    }

    fn load_sector(&mut self, sector: u64) -> Result<(), ()> {
        if self.buffer_sector == Some(sector) {
            return Ok(());
        }

        self.flush_buffer()?;

        let data = self.blk_dev.read_sectors(sector, 1).map_err(|_| ())?;

        self.sector_buffer
            .copy_from_slice(&data[..self.sector_size as usize]);
        self.buffer_sector = Some(sector);
        self.buffer_dirty = false;

        Ok(())
    }
}

impl fatfs::IoBase for MyFileSystem {
    type Error = ();
}

impl Read for MyFileSystem {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
        if buf.is_empty() {
            return Ok(0);
        }

        let mut bytes_read = 0;
        let mut remaining = buf.len();

        while remaining > 0 {
            let sector = self.position / self.sector_size;
            let sector_offset = (self.position % self.sector_size) as usize;

            self.load_sector(sector)?;

            let bytes_in_sector = self.sector_size as usize - sector_offset;
            let bytes_to_read = remaining.min(bytes_in_sector);

            let start = bytes_read;
            let end = start + bytes_to_read;
            buf[start..end]
                .copy_from_slice(&self.sector_buffer[sector_offset..sector_offset + bytes_to_read]);

            bytes_read += bytes_to_read;
            remaining -= bytes_to_read;
            self.position += bytes_to_read as u64;
        }

        Ok(bytes_read)
    }
}

impl Write for MyFileSystem {
    fn write(&mut self, buf: &[u8]) -> Result<usize, ()> {
        if buf.is_empty() {
            return Ok(0);
        }

        let mut bytes_written = 0;
        let mut remaining = buf.len();

        while remaining > 0 {
            let sector = self.position / self.sector_size;
            let sector_offset = (self.position % self.sector_size) as usize;

            self.load_sector(sector)?;

            let bytes_in_sector = self.sector_size as usize - sector_offset;
            let bytes_to_write = remaining.min(bytes_in_sector);

            let start = bytes_written;
            let end = start + bytes_to_write;
            self.sector_buffer[sector_offset..sector_offset + bytes_to_write]
                .copy_from_slice(&buf[start..end]);

            self.buffer_dirty = true;

            // 新增：如果设置了立即写入模式，立即执行写入
            if self.immediate_write {
                if let Some(current_sector) = self.buffer_sector {
                    self.blk_dev
                        .write_sectors(current_sector, &self.sector_buffer)
                        .map_err(|_| ())?;
                    self.buffer_dirty = false;
                }
            }

            bytes_written += bytes_to_write;
            remaining -= bytes_to_write;
            self.position += bytes_to_write as u64;
        }

        Ok(bytes_written)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.flush_buffer()?;
        Ok(())
    }
}

impl Seek for MyFileSystem {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64, Self::Error> {
        let new_position = match pos {
            SeekFrom::Start(offset) => offset,
            SeekFrom::End(offset) => {
                if offset >= 0 {
                    return Err(());
                }
                return Err(());
            }
            SeekFrom::Current(offset) => {
                if offset >= 0 {
                    self.position + offset as u64
                } else {
                    let abs_offset = (-offset) as u64;
                    if abs_offset > self.position {
                        return Err(());
                    }
                    self.position - abs_offset
                }
            }
        };

        self.position = new_position;
        Ok(self.position)
    }
}
