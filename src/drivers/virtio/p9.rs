use alloc::string::String;
use alloc::vec::Vec;
use bitflags::bitflags;
use lazy_static::lazy_static;
use log::{info, warn};
use virtio_drivers::{queue::VirtQueue, transport::mmio::MmioTransport, transport::Transport};
use crate::drivers::virtio::hal::VirtioHalImpl;
use crate::hal::Mutex;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct P9Feature: u64 {
        const RING_INDIRECT_DESC = 1 << 28;
        const RING_EVENT_IDX = 1 << 29;
        const VERSION_1 = 1 << 32;
    }
}

const QUEUE: u16 = 0;
const QUEUE_SIZE: u16 = 16;

pub struct VirtIOP9 {
    transport: MmioTransport<'static>,
    queue: VirtQueue<VirtioHalImpl, { QUEUE_SIZE as usize }>,
    mount_tag: String,
}

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

impl VirtIOP9 {
    pub fn new(mut transport: MmioTransport<'static>) -> Result<Self, virtio_drivers::Error> {
        let features = transport.begin_init(
            P9Feature::RING_INDIRECT_DESC | P9Feature::RING_EVENT_IDX | P9Feature::VERSION_1,
        );

        let queue = VirtQueue::new(
            &mut transport,
            QUEUE,
            features.contains(P9Feature::RING_INDIRECT_DESC),
            features.contains(P9Feature::RING_EVENT_IDX),
        )?;
        transport.finish_init();

        let mount_tag = read_mount_tag(&transport).unwrap_or_else(|| String::from("tools"));

        Ok(Self {
            transport,
            queue,
            mount_tag,
        })
    }

    pub fn mount_tag(&self) -> &str {
        &self.mount_tag
    }

    pub fn request(&mut self, req: &[u8], resp: &mut [u8]) -> Result<usize, virtio_drivers::Error> {
        if req.is_empty() || resp.len() < 7 {
            return Err(virtio_drivers::Error::InvalidParam);
        }
        let used_len = self
            .queue
            .add_notify_wait_pop(&[req], &mut [resp], &mut self.transport)?;

        let size = u32::from_le_bytes([resp[0], resp[1], resp[2], resp[3]]) as usize;
        warn!(
            "virtio-9p resp sizes: used_len={}, payload_len={}",
            used_len,
            size
        );
        Ok(size.min(resp.len()))
    }
}

fn read_mount_tag(transport: &MmioTransport<'static>) -> Option<String> {
    let tag_len: u16 = transport.read_config_space(0).ok()?;
    if tag_len == 0 {
        return None;
    }

    let mut bytes = Vec::with_capacity(tag_len as usize);
    for idx in 0..tag_len as usize {
        let b: u8 = transport.read_config_space(2 + idx).ok()?;
        bytes.push(b);
    }

    String::from_utf8(bytes).ok()
}
