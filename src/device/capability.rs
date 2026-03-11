//! OS-level capability facade for device access.

use core::any::Any;

use crate::{TinyResult, device::core::{DeviceInfo, InitLevel}};
use linkme::distributed_slice;


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
    pub probe: fn(&DeviceInfo) -> TinyResult<()>,
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
        #[linkme::distributed_slice($crate::device::capability::PROVIDERS)]
        static $name: $crate::device::capability::ProviderDescriptor =
            $crate::device::capability::ProviderDescriptor {
                vendor_id: $vendor_id,
                device_id: $device_id,
                priority: $priority,
                provider: &$ops,
                driver: Some($crate::device::capability::ProviderDriver {
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
        #[linkme::distributed_slice($crate::device::capability::PROVIDERS)]
        static $name: $crate::device::capability::ProviderDescriptor =
            $crate::device::capability::ProviderDescriptor {
                vendor_id: $vendor_id,
                device_id: $device_id,
                priority: $priority,
                provider: &$ops,
                driver: None,
            };
    };
    ($name:ident, $vendor_id:expr, $device_id:expr, $priority:expr, $ops:expr) => {
        $crate::define_provider!(
            provider: $name,
            vendor_id: $vendor_id,
            device_id: $device_id,
            priority: $priority,
            ops: $ops
        );
    };
}
