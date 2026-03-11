# Unified Driver Model - Design Sketch (MVP)

This sketch proposes a practical architecture for `arm_rstiny` that combines:
- Linux-like `discover -> register -> bind`
- Tock-like HIL capability traits
- Zephyr-like init levels

The goal is to improve decoupling and readability while keeping implementation effort low.

## 1. Core Concepts

### 1.1 Two orthogonal axes

- Capability axis: `OS capability -> HIL trait -> concrete driver`
- Lifecycle axis: `discover -> register -> bind`

This separates *what the OS needs* from *how hardware is found and attached*.

### 1.2 Layers

1. HIL capability traits (hardware-agnostic interfaces)
2. Device model (`DeviceInfo` from FDT/platform tables)
3. Driver model (`Driver::matches/probe`)
4. Driver manager (`DriverManager` handles registration and binding)

## 2. Proposed Directory Layout

```text
src/drivers/
  core/
    mod.rs
    model.rs      # DeviceInfo, Driver, InitLevel
    manager.rs    # DriverManager + registry + bind logic
    bus.rs        # Bus trait + FdtBus implementation
    macros.rs     # declare_driver!, register_driver!
  hil/
    mod.rs
    serial.rs     # SerialTx/SerialRx
    timer.rs      # ClockSource, OneShotTimer
    block.rs      # BlockDevice
  uart/
  timer/
  irq/
  virtio/
```

## 3. Minimal Type Sketch

```rust
// src/drivers/core/model.rs

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum InitLevel {
    Early,
    Core,
    Normal,
    Late,
}

#[derive(Clone, Copy, Debug)]
pub struct DeviceInfo<'a> {
    pub node_path: &'a str,
    pub compatible: &'a [&'a str],
    pub reg_base: Option<usize>,
    pub reg_size: Option<usize>,
    pub irq: Option<u32>,
}

pub trait Driver: Sync {
    fn name(&self) -> &'static str;
    fn init_level(&self) -> InitLevel {
        InitLevel::Normal
    }
    fn compatible(&self) -> &'static [&'static str];
    fn probe(&self, dev: &DeviceInfo) -> crate::TinyResult<()>;

    fn matches(&self, dev: &DeviceInfo) -> bool {
        dev.compatible
            .iter()
            .any(|c| self.compatible().iter().any(|k| k == c))
    }
}
```

## 4. Manager Sketch

```rust
// src/drivers/core/manager.rs

use crate::hal::Mutex;
use super::model::{DeviceInfo, Driver, InitLevel};

const MAX_DRIVERS: usize = 64;

pub struct DriverManager {
    drivers: Mutex<[Option<&'static dyn Driver>; MAX_DRIVERS]>,
}

impl DriverManager {
    pub const fn new() -> Self {
        Self { drivers: Mutex::new([None; MAX_DRIVERS]) }
    }

    pub fn register(&self, drv: &'static dyn Driver) {
        let mut guard = self.drivers.lock();
        for slot in guard.iter_mut() {
            if slot.is_none() {
                *slot = Some(drv);
                return;
            }
        }
        panic!("driver registry full");
    }

    pub fn bind_device(&self, dev: &DeviceInfo, level: InitLevel) {
        let guard = self.drivers.lock();
        for drv in guard.iter().flatten() {
            if drv.init_level() == level && drv.matches(dev) {
                if let Err(e) = drv.probe(dev) {
                    warn!("probe failed: {} on {}: {:?}", drv.name(), dev.node_path, e);
                }
            }
        }
    }
}
```

## 5. Bus and Discovery Sketch

```rust
// src/drivers/core/bus.rs

use alloc::vec::Vec;
use super::model::DeviceInfo;

pub trait Bus {
    fn enumerate<'a>(&'a self) -> Vec<DeviceInfo<'a>>;
}

pub struct FdtBus;

impl Bus for FdtBus {
    fn enumerate<'a>(&'a self) -> Vec<DeviceInfo<'a>> {
        let mut out = Vec::new();
        let fdt = crate::drivers::fdt::get_fdt().lock();
        for node in fdt.all_nodes() {
            let compatible = match node.compatible() {
                Some(c) => c,
                None => continue,
            };
            let comp_list: Vec<&str> = compatible.all().collect();
            let reg = node.reg().next();
            out.push(DeviceInfo {
                node_path: node.name,
                compatible: comp_list.leak(),
                reg_base: reg.map(|r| r.starting_address as usize),
                reg_size: reg.and_then(|r| r.size),
                irq: None,
            });
        }
        out
    }
}
```

Note: For the real implementation, avoid `Vec::leak` and own a stable arena/slab for compatibility arrays.

## 6. Macro Sketch

```rust
// src/drivers/core/macros.rs

#[macro_export]
macro_rules! declare_driver {
    ($ty:ty, $name:expr, $level:expr, [$($compat:expr),* $(,)?]) => {
        impl $crate::drivers::core::model::Driver for $ty {
            fn name(&self) -> &'static str { $name }
            fn init_level(&self) -> $crate::drivers::core::model::InitLevel { $level }
            fn compatible(&self) -> &'static [&'static str] { &[$($compat),*] }
            fn probe(
                &self,
                dev: &$crate::drivers::core::model::DeviceInfo,
            ) -> $crate::TinyResult<()> {
                Self::probe_impl(dev)
            }
        }
    };
}

#[macro_export]
macro_rules! register_driver {
    ($mgr:expr, $drv:expr) => {
        $mgr.register($drv as &'static dyn $crate::drivers::core::model::Driver)
    };
}
```

## 7. HIL Trait Sketch (Tock-style split)

```rust
// src/drivers/hil/serial.rs

pub trait SerialTx {
    fn write_bytes(&self, data: &[u8]) -> crate::TinyResult<usize>;
}

pub trait SerialRx {
    fn read_byte(&self) -> crate::TinyResult<u8>;
}

pub trait IrqSink {
    fn on_irq(&self, irq: usize);
}
```

Example: both `uart/pl011.rs` and `uart/dw_apb.rs` implement `SerialTx + SerialRx`.
`console/tty` depends only on these traits, not concrete UART modules.

## 8. Boot Integration Plan (for current codebase)

Current flow in `src/boot/mod.rs` should remain stable first:
- Keep manual early init for IRQ, timer, power, and early UART
- Keep FDT init

Then add manager-based flow:
1. Build `DriverManager` singleton
2. Register drivers in one place (`drivers/register_all.rs`)
3. Enumerate devices from `FdtBus`
4. Bind by init levels in order: `Early -> Core -> Normal -> Late`

Pseudo-flow:

```rust
pub fn driver_init() {
    register_all_drivers();
    let devices = FdtBus.enumerate();

    for level in [InitLevel::Early, InitLevel::Core, InitLevel::Normal, InitLevel::Late] {
        for dev in &devices {
            DRIVER_MANAGER.bind_device(dev, level);
        }
    }
}
```

## 9. Migration Strategy (low risk)

1. Step 1: introduce `core/model/manager/macros` without changing existing behavior
2. Step 2: migrate VirtIO MMIO scan+init into manager (`Normal` level)
3. Step 3: migrate UART to HIL traits for `console/tty` decoupling
4. Step 4: optionally migrate timer and power
5. Step 5: add optional linker-section auto registration (later)

## 10. Design Decisions and Trade-offs

- Choose explicit registration first: easier debug and deterministic order
- Keep `InitLevel`: avoids early-boot ordering bugs
- Keep traits small: better readability and testability
- Do not over-abstract buses initially: implement `FdtBus` first, extend later

## 11. What this model gives you

- Better decoupling: upper layers stop importing concrete chip drivers
- Better readability: each driver file declares capabilities + compatible list + probe
- Better scalability: adding new hardware mostly means adding one driver and one registration line
- Better testability: HIL traits allow mock-based tests for capsules/services
