use flat_device_tree::Fdt;
use lazyinit::LazyInit;

use crate::hal::Mutex;

static FDT_DATA: LazyInit<Mutex<Fdt>> = LazyInit::new();

pub fn fdt_init(fdt: usize) {
    let fdt = unsafe { Fdt::from_ptr(fdt as *const u8).unwrap() };

    FDT_DATA.init_once(Mutex::new(fdt));
}

pub fn get_fdt() -> &'static Mutex<Fdt<'static>> {
    FDT_DATA.get().expect("FDT not initialized")
}

crate::define_provider!(
    provider: BOOT_PROVIDER,
    vendor_id: 0,
    device_id: 0,
    priority: 100,
    ops: crate::device::provider::BootProvider {
        fdt_init,
        get_fdt,
        driver_init_early: crate::drivers::driver_init_early,
        driver_init: crate::drivers::driver_init,
    }
);
