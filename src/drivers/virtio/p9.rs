use lazy_static::lazy_static;
use log::{info, warn};
use virtio_drivers::device::virtio_9p::VirtIO9p;
use virtio_drivers::transport::mmio::MmioTransport;
use crate::drivers::virtio::hal::VirtioHalImpl;
use crate::hal::Mutex;

pub type VirtIOP9 = VirtIO9p<VirtioHalImpl, MmioTransport<'static>>;

lazy_static! {
    pub static ref P9_DEVICE: Mutex<Option<VirtIOP9>> = Mutex::new(None);
}

pub fn init(transport: MmioTransport<'static>) {
    match VirtIOP9::new(transport) {
        Ok(dev) => {
            info!("virtio-9p initialized (tag: {})", dev.mount_tag());
            *P9_DEVICE.lock() = Some(dev);
        }
        Err(err) => {
            warn!("virtio-9p init failed: {:?}", err);
        }
    }
}
