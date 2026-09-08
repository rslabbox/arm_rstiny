#![no_std]
#![no_main]
use core::sync::atomic::{AtomicU64, Ordering};
use rstiny::{debug_println, suspend_self, yield_now};
use rstiny_runtime::{BootInfo, entry};

// Application results also let the host verify a silent LOG=off boot.
#[unsafe(no_mangle)]
static BOOTINFO_ADDRESS: AtomicU64 = AtomicU64::new(0);
#[unsafe(no_mangle)]
static RESULT: AtomicU64 = AtomicU64::new(0);

#[entry]
fn main(info: &mut BootInfo) -> ! {
    BOOTINFO_ADDRESS.store(info.address() as u64, Ordering::Relaxed);
    debug_println!("[fatboot] root task started in EL0; BootInfo accepted");
    let tree = rs_fdtree::LinuxFdt::new(info.device_tree()).expect("invalid boot DTB");
    assert!(tree.find_compatible("arm,gic-v3").is_some());
    assert!(tree.find_compatible("arm,pl011").is_some());
    let psci = tree.find_compatible("arm,psci-1.0").expect("missing PSCI");
    assert!(matches!(psci.property_str("method"), Some("smc" | "hvc")));
    assert!(
        tree.find_node("/chosen")
            .unwrap()
            .property("seL4,kernel-devices")
            .is_some()
    );
    debug_println!("[fatboot] DTB parsed in EL0");
    yield_now().expect("yield failed");
    let buffer = info.ipc_buffer();
    assert_eq!(buffer[0], 0);
    buffer[0] = 0xface_cafe;
    let frames = rstiny::available_frames().expect("frame accounting");
    let child = rstiny::Task::create().expect("create child");
    assert_eq!(child.status().unwrap(), rstiny::TaskState::Created);
    child.destroy().expect("destroy child");
    assert_eq!(rstiny::available_frames().unwrap(), frames);
    let before = rstiny::clock_milliseconds().unwrap();
    rstiny::sleep(20).expect("timer wakeup");
    assert!(rstiny::clock_milliseconds().unwrap() - before >= 20);
    RESULT.store(1, Ordering::Relaxed);
    debug_println!("[fatboot] SVC round-trip passed; suspending");
    suspend_self()
}
