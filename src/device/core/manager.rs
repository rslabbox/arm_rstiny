//! Driver manager with explicit registration and level-based binding.

use crate::hal::Mutex;
use crate::device::capability::PROVIDERS;

use super::model::{DeviceInfo, InitLevel};

const MAX_BINDINGS: usize = 256;

#[derive(Clone, Copy)]
struct BindingRecord {
    driver_name: &'static str,
    node_hash: u64,
}

pub struct DriverManager {
    bindings: Mutex<[Option<BindingRecord>; MAX_BINDINGS]>,
}

impl DriverManager {
    pub const fn new() -> Self {
        Self {
            bindings: Mutex::new([None; MAX_BINDINGS]),
        }
    }

    pub fn bind_device_for_level(&self, dev: &DeviceInfo, level: InitLevel) {
        for provider in PROVIDERS.iter() {
            let Some(driver) = provider.driver else {
                continue;
            };

            if driver.level != level
                || !driver.compatibles.iter().any(|compatible| dev.has_compatible(compatible))
            {
                continue;
            }

            let node_hash = hash_node(dev.node_name);
            if self.is_bound(driver.name, node_hash) {
                continue;
            }

            if let Err(err) = (driver.probe)(dev) {
                warn!(
                    "driver probe failed: driver={} node={} err={:?}",
                    driver.name,
                    dev.node_name,
                    err
                );
                continue;
            }

            self.record_binding(driver.name, node_hash);
        }
    }

    fn is_bound(&self, driver_name: &'static str, node_hash: u64) -> bool {
        let guard = self.bindings.lock();
        guard
            .iter()
            .flatten()
            .any(|r| r.driver_name == driver_name && r.node_hash == node_hash)
    }

    fn record_binding(&self, driver_name: &'static str, node_hash: u64) {
        let mut guard = self.bindings.lock();
        for slot in guard.iter_mut() {
            if slot.is_none() {
                *slot = Some(BindingRecord {
                    driver_name,
                    node_hash,
                });
                return;
            }
        }
        warn!("binding table full, skipping record for {}", driver_name);
    }
}

fn hash_node(name: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in name.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

static DRIVER_MANAGER: DriverManager = DriverManager::new();

pub fn driver_manager() -> &'static DriverManager {
    &DRIVER_MANAGER
}
