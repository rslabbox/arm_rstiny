#![no_std]
#![no_main]
use rstiny_runtime::entry;

#[entry]
fn main() -> ! {
    rstiny::debug_println!("[hello] Hello, world!");
    rstiny::exit(0)
}
