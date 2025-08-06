
mod gicv2;
pub use self::imp::handle_irq;
use gicv2 as imp;

#[allow(unused_imports)]
pub(super) use self::imp::{init, register_handler, set_enable};
