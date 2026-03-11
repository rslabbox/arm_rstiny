//! Unified provider registry for OS device capabilities.

use core::time::Duration;

use arm_gic::IntId;
use memory_addr::VirtAddr;
use provider_macros::capability_provider;

use crate::TinyResult;

#[derive(Clone, Copy)]
#[capability_provider]
pub struct UartProvider {
    pub init_early: fn(base: VirtAddr, irq: IntId),
    pub puts: fn(message: &str),
    pub putchar: fn(byte: u8),
    pub getchar: fn() -> Option<u8>,
}

#[derive(Clone, Copy)]
#[capability_provider]
pub struct IrqProvider {
    pub init: fn(gicd_base: VirtAddr, gicr_base: VirtAddr) -> TinyResult<()>,
    pub init_secondary: fn(cpu_id: usize),
    pub register: fn(intid: IntId, handler: fn(usize)),
    pub enable: fn(intid: IntId, priority: u8),
    pub handle: fn(),
}

#[derive(Clone, Copy)]
#[capability_provider]
pub struct TimerProvider {
    pub boot_nanoseconds: fn() -> u64,
    pub nanos_per_sec: fn() -> u64,
    pub current_nanoseconds: fn() -> u64,
    pub busy_wait: fn(duration: Duration),
    pub init_secondary: fn(),
}

#[derive(Clone, Copy)]
#[capability_provider]
pub struct PowerProvider {
    pub init: fn(method: &'static str) -> TinyResult<()>,
    pub cpu_on: fn(cpu_id: usize, entry: usize, context_id: usize),
    pub system_off: fn() -> !,
}

#[derive(Clone, Copy)]
#[capability_provider]
pub struct BootProvider {
    pub fdt_init: fn(fdt: usize),
    pub get_fdt: fn() -> &'static crate::hal::Mutex<flat_device_tree::Fdt<'static>>,
    pub driver_init_early: fn(),
    pub driver_init: fn(),
}

#[derive(Clone, Copy)]
#[capability_provider]
pub struct BlockProvider {
    pub read_blocks: fn(block_id: usize, dst: &mut [u8]) -> TinyResult<()>,
    pub write_blocks: fn(block_id: usize, src: &[u8]) -> TinyResult<()>,
    pub capacity_blocks: fn() -> TinyResult<u64>,
}
