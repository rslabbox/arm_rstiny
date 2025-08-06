pub mod interrupt;
pub mod misc;
pub mod timer;


pub fn init() {
    println!("Initializing drivers...");
    interrupt::init();
    timer::init();
}
