#![no_std]
#![no_main]
//! hello: the disk-loaded demonstration app. Announces itself to its
//! supervisor (appmgr) through the standard service protocol, prints through
//! the console service and exits cleanly — its ELF comes from the FAT32 disk,
//! not from the system image (docs/disk-driver.md section 10).
use rstiny_protocol::Argument;
use rstiny_runtime::entry;
use rstiny_server::{Service, logln};

#[entry]
fn main(argument: Argument) -> ! {
    let Some(service) = Service::init(argument) else {
        loop {
            core::hint::spin_loop();
        }
    };
    // HELLO_MSG lets the acceptance swap the on-disk ELF's wording without a
    // second crate: the same manifest, a different binary, different output.
    let message = option_env!("HELLO_MSG").unwrap_or("[hello] Hello, world! (loaded from disk)");
    logln!(service, "{}", message);
    service.exit(0)
}
