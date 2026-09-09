#![no_std]
#![no_main]
use core::sync::atomic::{AtomicU64, Ordering};
use rstiny::{TaskState, debug_println, suspend_self};
use rstiny_runtime::{BootInfo, entry};

include!(concat!(env!("OUT_DIR"), "/hello.rs"));
unsafe extern "C" {
    static __hello_start: u8;
    static __hello_end: u8;
}

// Host-visible completion also verifies a silent LOG=off boot.
#[unsafe(no_mangle)]
static BOOTINFO_ADDRESS: AtomicU64 = AtomicU64::new(0);
#[unsafe(no_mangle)]
static RESULT: AtomicU64 = AtomicU64::new(0);

#[entry(stack_size = 32 * 1024)]
fn main(info: &mut BootInfo) -> ! {
    BOOTINFO_ADDRESS.store(info.address() as u64, Ordering::Relaxed);
    check_capabilities();
    debug_println!("[fatboot] loading hello.elf");
    // Linker-owned immutable resource; it contains a separate executable ELF.
    let image = unsafe {
        let start = core::ptr::addr_of!(__hello_start);
        core::slice::from_raw_parts(
            start,
            core::ptr::addr_of!(__hello_end) as usize - start as usize,
        )
    };
    // The initial metadata bounds identify an unused page owned by this root.
    let child =
        unsafe { rstiny::elf::spawn(image, info.first_free_address()) }.expect("load hello.elf");
    let code = child.wait().expect("wait hello");
    assert_eq!(child.status().expect("hello status"), TaskState::Exited);
    assert_eq!(code, 0, "hello failed");
    child.destroy().expect("reap hello");
    RESULT.store(1, Ordering::Relaxed);
    debug_println!("[fatboot] hello.elf exited successfully");
    suspend_self()
}

// Exercise the standard object interface before using the managed loader.
fn check_capabilities() {
    use rstiny::capability::*;
    let free = rstiny::available_frames().unwrap();
    let untyped = Untyped(CPtr(INIT_UNTYPED));
    let cnode = CNode(CPtr(INIT_CNODE));
    let table = PageTable(CPtr(32));
    let page = Page(CPtr(33));
    let alias = Page(CPtr(34));
    let address = 0x0700_0000;
    untyped
        .retype(ObjectType::PageTable, 0, CPtr(INIT_CNODE), 32, 1)
        .unwrap();
    untyped
        .retype(ObjectType::SmallPage, 0, CPtr(INIT_CNODE), 33, 1)
        .unwrap();
    table.map(CPtr(INIT_VSPACE), address).unwrap();
    cnode.copy(34, CPtr(INIT_CNODE), 33, RIGHTS_READ).unwrap();
    // This scratch range and the capability slots are exclusively owned here.
    unsafe {
        page.map(
            CPtr(INIT_VSPACE),
            address,
            RIGHTS_READ | RIGHTS_WRITE,
            VM_CACHEABLE | VM_EXECUTE_NEVER,
        )
        .unwrap();
        alias
            .map(
                CPtr(INIT_VSPACE),
                address + 4096,
                RIGHTS_READ | RIGHTS_WRITE,
                VM_CACHEABLE | VM_EXECUTE_NEVER,
            )
            .unwrap();
        (address as *mut u64).write_volatile(0x1234_5678);
        assert_eq!(
            ((address + 4096) as *const u64).read_volatile(),
            0x1234_5678
        );
        cnode.revoke(33).unwrap();
        assert_eq!(alias.unmap(), Err(rstiny::Error::FailedLookup));
        page.unmap().unwrap();
        cnode.delete(33).unwrap();
        cnode.delete(32).unwrap();
    }
    assert_eq!(rstiny::available_frames().unwrap(), free);
}
