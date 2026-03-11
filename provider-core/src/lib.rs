#![no_std]

use core::any::Any;

use linkme::distributed_slice;

pub type TinyResult<T> = anyhow::Result<T>;

pub const MAX_COMPAT_ENTRIES: usize = 8;

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum InitLevel {
    Early,
    Core,
    Normal,
    Late,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub struct DeviceInfo<'a> {
    pub node_name: &'a str,
    pub compatible: [Option<&'a str>; MAX_COMPAT_ENTRIES],
    pub reg_base: Option<usize>,
    pub reg_size: Option<usize>,
    pub irq: Option<u32>,
}

impl<'a> DeviceInfo<'a> {
    pub fn has_compatible(&self, target: &str) -> bool {
        self.compatible.iter().flatten().any(|c| *c == target)
    }
}

pub trait CapabilityProvider {
    type Handle;

    fn resolve() -> Self::Handle;
}

pub fn with_provider<P>() -> P::Handle
where
    P: CapabilityProvider,
{
    P::resolve()
}

pub trait RegisteredProvider: Sync {
    fn as_any(&self) -> &dyn Any;
}

impl<T> RegisteredProvider for T
where
    T: Any + Sync,
{
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy)]
pub struct ProviderDriver {
    pub name: &'static str,
    pub level: InitLevel,
    pub compatibles: &'static [&'static str],
    pub probe: for<'a> fn(&DeviceInfo<'a>) -> TinyResult<()>,
}

#[allow(dead_code)]
#[derive(Clone, Copy)]
pub struct ProviderDescriptor {
    pub vendor_id: u64,
    pub device_id: u64,
    pub priority: u32,
    pub provider: &'static dyn RegisteredProvider,
    pub driver: Option<ProviderDriver>,
}

#[distributed_slice]
pub static PROVIDERS: [ProviderDescriptor];

#[macro_export]
macro_rules! define_provider {
    (
        provider: $name:ident,
        vendor_id: $vendor_id:expr,
        device_id: $device_id:expr,
        priority: $priority:expr,
        ops: $ops:expr,
        driver: {
            name: $driver_name:expr,
            level: $level:expr,
            compatibles: [$($compat:expr),+ $(,)?],
            probe: $probe:path $(,)?
        } $(,)?
    ) => {
        #[linkme::distributed_slice($crate::PROVIDERS)]
        static $name: $crate::ProviderDescriptor = $crate::ProviderDescriptor {
            vendor_id: $vendor_id,
            device_id: $device_id,
            priority: $priority,
            provider: &$ops,
            driver: Some($crate::ProviderDriver {
                name: $driver_name,
                level: $level,
                compatibles: &[$($compat),+],
                probe: $probe,
            }),
        };
    };
    (
        provider: $name:ident,
        vendor_id: $vendor_id:expr,
        device_id: $device_id:expr,
        priority: $priority:expr,
        ops: $ops:expr $(,)?
    ) => {
        #[linkme::distributed_slice($crate::PROVIDERS)]
        static $name: $crate::ProviderDescriptor = $crate::ProviderDescriptor {
            vendor_id: $vendor_id,
            device_id: $device_id,
            priority: $priority,
            provider: &$ops,
            driver: None,
        };
    };
    ($name:ident, $vendor_id:expr, $device_id:expr, $priority:expr, $ops:expr) => {
        $provider_core::define_provider!(
            provider: $name,
            vendor_id: $vendor_id,
            device_id: $device_id,
            priority: $priority,
            ops: $ops
        );
    };
}
