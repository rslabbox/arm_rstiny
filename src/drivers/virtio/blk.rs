use crate::TinyResult;
use crate::device::core::{DeviceInfo, InitLevel};
use crate::drivers::virtio::hal::VirtioHalImpl;
use crate::hal::Mutex;
use core::ptr::NonNull;
use lazy_static::lazy_static;
use log::info;
use virtio_drivers::transport::{DeviceType, Transport, mmio::VirtIOHeader};
use virtio_drivers::{device::blk::VirtIOBlk, transport::mmio::MmioTransport};

type VirtioBlkDevice = VirtIOBlk<VirtioHalImpl, MmioTransport<'static>>;

lazy_static! {
    pub static ref BLOCK_DEVICE: Mutex<Option<VirtioBlkDevice>> = Mutex::new(None);
}

pub fn with_block_device<R>(
    f: impl FnOnce(&mut VirtioBlkDevice) -> TinyResult<R>,
) -> TinyResult<R> {
    let mut guard = BLOCK_DEVICE.lock();
    let dev = guard
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("block device not initialized"))?;
    f(dev)
}

fn block_read_blocks(block_id: usize, dst: &mut [u8]) -> TinyResult<()> {
    with_block_device(|dev| {
        VirtIOBlk::read_blocks(dev, block_id, dst)
            .map_err(|e| anyhow::anyhow!("virtio-blk read failed: {:?}", e))
    })
}

fn block_write_blocks(block_id: usize, src: &[u8]) -> TinyResult<()> {
    with_block_device(|dev| {
        VirtIOBlk::write_blocks(dev, block_id, src)
            .map_err(|e| anyhow::anyhow!("virtio-blk write failed: {:?}", e))
    })
}

fn block_capacity_blocks() -> TinyResult<u64> {
    with_block_device(|dev| Ok(dev.capacity()))
}

fn probe(dev: &DeviceInfo) -> TinyResult<()> {
    let (Some(base), Some(size)) = (dev.reg_base, dev.reg_size) else {
        return Ok(());
    };

    if size <= core::mem::size_of::<VirtIOHeader>() {
        return Ok(());
    }

    let Some(header) = NonNull::new(base as *mut VirtIOHeader) else {
        return Ok(());
    };

    let transport = match unsafe { MmioTransport::new(header, size) } {
        Ok(transport) => transport,
        Err(_) => return Ok(()),
    };

    if transport.device_type() != DeviceType::Block {
        return Ok(());
    }

    let blk = VirtIOBlk::new(transport).expect("failed to create blk driver");
    *BLOCK_DEVICE.lock() = Some(blk);
    info!("virtio-blk initialized");
    Ok(())
}

provider_core::define_provider!(
    provider: BLOCK_PROVIDER,
    vendor_id: 0x1af4,
    device_id: 2,
    priority: 100,
    ops: crate::device::provider::BlockProvider {
        read_blocks: block_read_blocks,
        write_blocks: block_write_blocks,
        capacity_blocks: block_capacity_blocks,
    },
    driver: {
        name: "virtio-blk",
        level: InitLevel::Normal,
        compatibles: ["virtio,mmio"],
        probe: probe,
    }
);
