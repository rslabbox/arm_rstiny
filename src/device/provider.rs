//! Unified provider registry for OS device capabilities.

use core::time::Duration;

use arm_gic::IntId;
use memory_addr::VirtAddr;
use provider_macros::capability_provider;

use crate::TinyResult;

#[derive(Clone, Copy)]
#[capability_provider]
pub struct UartProvider {
    pub init_early: fn(VirtAddr, IntId),
    pub puts: fn(&str),
    pub putchar: fn(u8),
    pub getchar: fn() -> Option<u8>,
}

#[derive(Clone, Copy)]
#[capability_provider]
pub struct IrqProvider {
    pub init: fn(VirtAddr, VirtAddr) -> TinyResult<()>,
    pub init_secondary: fn(usize),
    pub handle: fn(),
}

#[derive(Clone, Copy)]
#[capability_provider]
pub struct TimerProvider {
    pub boot_nanoseconds: fn() -> u64,
    pub nanos_per_sec: fn() -> u64,
    pub current_nanoseconds: fn() -> u64,
    pub busy_wait: fn(Duration),
    pub init_secondary: fn(),
}

#[derive(Clone, Copy)]
#[capability_provider]
pub struct PowerProvider {
    pub init: fn(&'static str) -> TinyResult<()>,
    pub cpu_on: fn(usize, usize, usize),
    pub system_off: fn() -> !,
}

#[derive(Clone, Copy)]
#[capability_provider]
pub struct BootProvider {
    pub fdt_init: fn(usize),
    pub get_fdt: fn() -> &'static crate::hal::Mutex<flat_device_tree::Fdt<'static>>,
    pub driver_init_early: fn(),
    pub driver_init: fn(),
}

#[derive(Clone, Copy)]
#[capability_provider]
pub struct BlockProvider {
    pub read_blocks: fn(usize, &mut [u8]) -> TinyResult<()>,
    pub write_blocks: fn(usize, &[u8]) -> TinyResult<()>,
    pub capacity_blocks: fn() -> TinyResult<u64>,
}
