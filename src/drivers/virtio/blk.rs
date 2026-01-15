use lazy_static::lazy_static;
use log::info;
use virtio_drivers::{device::blk::VirtIOBlk, transport::mmio::MmioTransport};
use crate::hal::Mutex;
use crate::drivers::virtio::hal::VirtioHalImpl;

type BlockDevice = VirtIOBlk<VirtioHalImpl, MmioTransport<'static>>;

lazy_static! {
    pub static ref BLOCK_DEVICE: Mutex<Option<BlockDevice>> = Mutex::new(None);
}

/// Initialize the VirtIO Block device driver.
/// This function is called from the virtio driver discovery loop.
pub fn init(transport: MmioTransport<'static>) {
    let blk = VirtIOBlk::new(transport).expect("failed to create blk driver");
    *BLOCK_DEVICE.lock() = Some(blk);
    info!("virtio-blk initialized");
}