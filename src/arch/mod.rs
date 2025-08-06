mod context;
mod page_table;
mod trap;

pub mod config;
pub mod instructions;

pub use self::context::{TrapFrame};
pub use self::page_table::{ PageTableEntry};

pub fn init_percpu() {
    trap::init();
}
